//! The real `System` for the host component: Windows locally, a WSL distro as root, resources
//! through PowerShell.
//!
//! Routing is by path: an absolute `/unix/path` belongs to the distro (reached with
//! `wsl.exe -d <distro> -u root --exec ...`, no shell and no password), any other path is a
//! Windows path handled by [`LocalSystem`]. Variables and the registry are always local.
//! Resources are dispatched by kind: firewall rules, Hyper-V firewall rules and scheduled tasks
//! through PowerShell; `WslPackage` (apt) and `WslUnit` (systemd) inside the distro.
//!
//! Every external command goes through a [`Runner`], so the logic is tested off Windows with a
//! scripted fake.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use goway_journal::{LocalSystem, RegValue, ResourceKind, SysResult, System, SystemError};

use crate::helper::{HOST_KEY_FILE, WslProbe, parse_fingerprint};
use crate::host::{FirewallSpec, HostFacts, HyperVSpec, TaskSpec};
use crate::native::{
    self, BuiltinRule, KeyAccount, NativeFacts, ScopeSpec, ServiceSpec, StartType,
};
use crate::ps;
use crate::relay::{self, AdapterAddr, PortProxyRule, PortProxySpec, RelayTaskSpec};
use crate::sysapi::{Tool, tool_path};

/// One external command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Program to start.
    pub program: String,
    /// Arguments, passed without any shell.
    pub args: Vec<String>,
    /// Bytes to feed to standard input.
    pub stdin: Option<Vec<u8>>,
}

/// What an external command produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Exit code (`None` when killed by a signal).
    pub code: Option<i32>,
    /// Standard output bytes.
    pub stdout: Vec<u8>,
    /// Standard error bytes.
    pub stderr: Vec<u8>,
}

impl Output {
    /// Whether the command exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Standard output as trimmed text (invalid UTF-8 replaced, NULs from UTF-16 tools dropped).
    pub fn text(&self) -> String {
        clean(&self.stdout)
    }

    /// Standard error as trimmed text.
    pub fn error_text(&self) -> String {
        clean(&self.stderr)
    }
}

fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace('\0', "")
        .trim()
        .to_owned()
}

/// Runs external commands; faked in tests.
pub trait Runner {
    /// Run `inv` to completion.
    fn run(&self, inv: &Invocation) -> std::io::Result<Output>;
}

/// Runs commands as real child processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, inv: &Invocation) -> std::io::Result<Output> {
        tracing::debug!(program = %inv.program, args = ?inv.args, stdin = inv.stdin.as_ref().map(Vec::len), "run command");
        let mut command = Command::new(&inv.program);
        command
            .args(&inv.args)
            .stdin(if inv.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn()?;
        if let (Some(bytes), Some(mut pipe)) = (inv.stdin.as_deref(), child.stdin.take()) {
            // Feed on a thread so a command that fills its output pipe cannot deadlock us.
            let bytes = bytes.to_vec();
            let writer = std::thread::spawn(move || pipe.write_all(&bytes));
            let out = child.wait_with_output()?;
            writer
                .join()
                .map_err(|_| std::io::Error::other("stdin writer panicked"))??;
            return Ok(logged(to_output(out)));
        }
        Ok(logged(to_output(child.wait_with_output()?)))
    }
}

fn logged(out: Output) -> Output {
    tracing::debug!(code = ?out.code, stdout = %out.text(), stderr = %out.error_text(), "command finished");
    out
}

fn to_output(out: std::process::Output) -> Output {
    Output {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}

/// Refuse a PowerShell-managed resource whose name could act as a wildcard pattern.
fn reject_wildcard(kind: ResourceKind, name: &str) -> SysResult<()> {
    let powershell = matches!(
        kind,
        ResourceKind::FirewallRule
            | ResourceKind::HyperVFirewallRule
            | ResourceKind::ScheduledTask
            | ResourceKind::FirewallScope
            | ResourceKind::WindowsCapability
    );
    if powershell && crate::host::has_wildcard(name) {
        tracing::error!(
            ?kind,
            name,
            "refusing a resource name with wildcard characters"
        );
        return Err(SystemError::InvalidState(format!(
            "resource name {name:?} contains wildcard characters"
        )));
    }
    Ok(())
}

/// Whether a path names something inside the distro (an absolute unix path).
pub fn is_wsl_path(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// How dpkg knows a package, as far as goway is willing to act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgState {
    /// Not installed and no configuration left behind.
    Absent,
    /// Cleanly installed (`ii`).
    Installed,
}

/// What activation did to the running sshd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// sshd already listened as configured; its configuration was only reloaded.
    Reloaded,
    /// sshd (or its socket) was restarted to pick up the listening ports.
    Restarted,
}

/// The host component's `System`.
#[derive(Debug)]
pub struct HostSystem<R: Runner = ProcessRunner> {
    local: LocalSystem,
    runner: R,
    distro: String,
    /// Never start the distro: it is only touched while `wsl.exe --list --running` shows it
    /// (set for an elevated process, whose token WSL interop would otherwise inherit).
    never_start: bool,
}

impl HostSystem<ProcessRunner> {
    /// A system acting on `distro` through real processes; an elevated process never starts the
    /// distro (see [`HostSystem::never_start_wsl`]).
    pub fn new(distro: &str) -> Self {
        Self::with_runner(distro, ProcessRunner)
            .never_start_wsl(cfg!(windows) && crate::elevate::is_elevated())
    }
}

fn cmd_error(what: &str, out: &Output) -> SystemError {
    SystemError::Command {
        what: what.to_owned(),
        detail: format!("exit {:?}: {}", out.code, out.error_text()),
    }
}

/// The path of a system tool for a scheduled task, which must be absolute: a bare name would be
/// searched for through directories an ordinary user can write. Off Windows the bare names the
/// tests see are accepted.
fn absolute_tool(tool: Tool) -> SysResult<String> {
    let path = tool_path(tool);
    if cfg!(windows) && !Path::new(&path).is_absolute() {
        return Err(SystemError::InvalidState(format!(
            "could not locate {tool:?} by absolute path (got {path:?}); refusing to register a task that searches for it"
        )));
    }
    Ok(path)
}

/// A local file error, with the refusal to follow a link kept distinguishable.
fn local_io(path: &Path, e: std::io::Error) -> SystemError {
    if crate::safefile::is_link_refusal(&e) {
        return SystemError::InvalidState(e.to_string());
    }
    SystemError::Io {
        path: path.to_path_buf(),
        source: e,
    }
}

fn spawn_error(what: &str, e: &std::io::Error) -> SystemError {
    SystemError::Command {
        what: what.to_owned(),
        detail: e.to_string(),
    }
}

impl<R: Runner> HostSystem<R> {
    /// A system acting on `distro` through `runner`.
    pub fn with_runner(distro: &str, runner: R) -> Self {
        Self {
            local: LocalSystem,
            runner,
            distro: distro.to_owned(),
            never_start: false,
        }
    }

    /// Whether to refuse every distro command while the distro is not running, so goway never
    /// starts WSL itself: WSL interop runs Windows programs with the token of whatever started
    /// WSL, and an elevated goway would hand administrator rights to every WSL user.
    #[must_use]
    pub fn never_start_wsl(mut self, on: bool) -> Self {
        self.never_start = on;
        self
    }

    /// Whether `wsl.exe --list --running` shows the distro (never starts it).
    pub fn distro_running(&self) -> SysResult<bool> {
        let inv = Invocation {
            program: tool_path(Tool::Wsl),
            args: ["--list", "--running", "--quiet"]
                .map(str::to_owned)
                .to_vec(),
            stdin: None,
        };
        let out = self.run("list running WSL distros", &inv)?;
        Ok(parse_distro_list(&out.text())
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&self.distro)))
    }

    /// Fail unless the distro may be touched: always when not guarded, else only while running.
    fn ensure_startable(&self) -> SysResult<()> {
        if !self.never_start || self.distro_running()? {
            return Ok(());
        }
        tracing::error!(distro = %self.distro, "refusing to start WSL from an elevated process");
        Err(SystemError::InvalidState(format!(
            "WSL distro {0} is not running, and goway never starts WSL from an elevated process (WSL interop would run Windows programs as administrator for every user of the distro). Start it from a normal, non-administrator terminal (`wsl -d {0} --exec true`), then run this again",
            self.distro
        )))
    }

    fn run(&self, what: &str, inv: &Invocation) -> SysResult<Output> {
        self.runner.run(inv).map_err(|e| spawn_error(what, &e))
    }

    /// The `wsl.exe` invocation running `argv` in the distro (as root unless `as_user`).
    fn wsl_invocation(&self, argv: &[&str], as_user: bool, stdin: Option<&[u8]>) -> Invocation {
        let mut full = vec!["-d".to_owned(), self.distro.clone()];
        if !as_user {
            full.extend(["-u".to_owned(), "root".to_owned()]);
        }
        full.push("--exec".to_owned());
        full.extend(argv.iter().map(|a| (*a).to_owned()));
        Invocation {
            program: tool_path(Tool::Wsl),
            args: full,
            stdin: stdin.map(<[u8]>::to_vec),
        }
    }

    /// Run `argv` as root inside the distro and return its output whatever the exit code.
    fn wsl_raw(&self, argv: &[&str], stdin: Option<&[u8]>) -> SysResult<Output> {
        self.ensure_startable()?;
        self.run(
            &format!("wsl {}", argv.join(" ")),
            &self.wsl_invocation(argv, false, stdin),
        )
    }

    /// Run `argv` inside the distro and require exit status 0.
    fn wsl(&self, argv: &[&str]) -> SysResult<Output> {
        let out = self.wsl_raw(argv, None)?;
        if out.success() {
            Ok(out)
        } else {
            Err(cmd_error(&format!("wsl {}", argv.join(" ")), &out))
        }
    }

    /// Run `argv` inside the distro as the distro's default user (no `-u root`).
    fn wsl_user(&self, argv: &[&str]) -> SysResult<Output> {
        self.ensure_startable()?;
        self.run(
            &format!("wsl (user) {}", argv.join(" ")),
            &self.wsl_invocation(argv, true, None),
        )
    }

    /// Whether `test <flag> <path>` holds inside the distro.
    fn wsl_test(&self, flag: &str, path: &str) -> SysResult<bool> {
        Ok(self.wsl_raw(&["test", flag, path], None)?.success())
    }

    fn powershell(&self, what: &str, script: &str) -> SysResult<Output> {
        let inv = Invocation {
            program: tool_path(Tool::PowerShell),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-EncodedCommand".into(),
                ps::encode_command(script),
            ],
            stdin: None,
        };
        let out = self.run(what, &inv)?;
        if out.success() {
            Ok(out)
        } else {
            Err(cmd_error(what, &out))
        }
    }

    /// Run a PowerShell probe that prints `1` or `0`.
    fn ps_flag(&self, what: &str, script: &str) -> SysResult<bool> {
        Ok(self.powershell(what, script)?.text() == "1")
    }

    fn spec<T: serde::de::DeserializeOwned>(
        kind: ResourceKind,
        name: &str,
        spec: &str,
    ) -> SysResult<T> {
        serde_json::from_str(spec)
            .map_err(|e| SystemError::InvalidState(format!("bad {kind:?} spec for {name}: {e}")))
    }

    /// The sshd service resource's spec from its name; refuses any other service.
    fn sshd_spec(name: &str) -> SysResult<ServiceSpec> {
        ServiceSpec::from_resource_name(name).ok_or(SystemError::Unsupported(
            "services other than the sshd resource goway makes",
        ))
    }

    /// Refuse every capability but the OpenSSH Server.
    fn only_capability(name: &str) -> SysResult<()> {
        if name == native::CAPABILITY {
            Ok(())
        } else {
            Err(SystemError::InvalidState(format!(
                "{name:?} is not the capability goway installs"
            )))
        }
    }

    /// Refuse every firewall rule but sshd's built-in one.
    fn only_builtin_rule(name: &str) -> SysResult<()> {
        if name == native::BUILTIN_RULE {
            Ok(())
        } else {
            Err(SystemError::InvalidState(format!(
                "{name:?} is not the built-in rule goway narrows"
            )))
        }
    }

    /// Parse a portproxy resource name (`<listen address>:<port>`).
    fn relay_name(name: &str) -> SysResult<(std::net::Ipv4Addr, u16)> {
        relay::parse_relay_name(name)
            .ok_or_else(|| SystemError::InvalidState(format!("bad portproxy name {name:?}")))
    }

    fn wsl_str(path: &Path) -> SysResult<&str> {
        path.to_str()
            .ok_or_else(|| SystemError::InvalidState(format!("non-UTF-8 path {}", path.display())))
    }

    // ----- facts and activation (not journaled) -----

    /// Whether the Hyper-V firewall cmdlets exist.
    pub fn hyperv_firewall_available(&self) -> SysResult<bool> {
        self.ps_flag("probe Hyper-V firewall", &ps::hyperv_available())
    }

    /// Names of the connected networks Windows classifies as Public (where the goway firewall
    /// rules do not apply); empty when none or when the query is unavailable.
    pub fn public_networks(&self) -> SysResult<Vec<String>> {
        let out = self.powershell("query network profiles", &ps::public_networks())?;
        Ok(out
            .text()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Whether the distro answers (exists and starts).
    pub fn distro_reachable(&self) -> SysResult<bool> {
        Ok(self.wsl_raw(&["true"], None)?.success())
    }

    /// The ed25519 ssh host key fingerprint (`SHA256:...`) of the distro's sshd; `None` when the
    /// key does not exist yet.
    pub fn host_key_fingerprint(&self) -> SysResult<Option<String>> {
        let out = self.wsl_raw(&["ssh-keygen", "-lf", HOST_KEY_FILE], None)?;
        if !out.success() {
            tracing::warn!(error = %out.error_text(), "no ssh host key fingerprint");
            return Ok(None);
        }
        Ok(parse_fingerprint(&out.text()))
    }

    /// The distro's default Linux user (the one whose password goway asks for).
    pub fn default_user(&self) -> SysResult<String> {
        let out = self.wsl_user(&["id", "-un"])?;
        if out.success() && !out.text().is_empty() {
            Ok(out.text())
        } else {
            Err(cmd_error("wsl id -un", &out))
        }
    }

    /// Whether systemd runs as PID 1 in the distro.
    pub fn systemd_running(&self) -> SysResult<bool> {
        let out = self.wsl_raw(&["ps", "-p", "1", "-o", "comm="], None)?;
        Ok(out.success() && out.text() == "systemd")
    }

    /// Ports sshd is configured to listen on (`sshd -T`); empty when sshd is not installed.
    pub fn sshd_ports(&self) -> SysResult<Vec<u16>> {
        if !self.wsl_test("-x", "/usr/sbin/sshd")? {
            return Ok(Vec::new());
        }
        let out = self.wsl(&["/usr/sbin/sshd", "-T"])?;
        Ok(parse_sshd_ports(&out.text()))
    }

    /// Whether the distro's default user has a non-empty `~/.ssh/authorized_keys`.
    pub fn default_user_has_authorized_keys(&self) -> SysResult<bool> {
        let out = self.wsl_user(&["printenv", "HOME"])?;
        if !out.success() || out.text().is_empty() {
            return Ok(false);
        }
        let path = format!("{}/.ssh/authorized_keys", out.text());
        self.wsl_test("-s", &path)
    }

    /// The Windows build number; `None` when the query fails or answers something else.
    pub fn windows_build(&self) -> Option<u32> {
        match self.powershell("query Windows build", &ps::windows_build()) {
            Ok(out) => out.text().parse().ok(),
            Err(e) => {
                tracing::warn!(error = %e, "could not read the Windows build");
                None
            }
        }
    }

    /// Whether the invoking account belongs to Administrators (also with a UAC-filtered token);
    /// `false` when it cannot be told.
    pub fn admin_account(&self) -> bool {
        match self.powershell("query administrator membership", &ps::admin_account()) {
            Ok(out) => out.text().eq_ignore_ascii_case("true"),
            Err(e) => {
                tracing::warn!(error = %e, "could not tell whether the account is an administrator");
                false
            }
        }
    }

    /// Whether the distro's `/etc/wsl.conf` sets `[interop] enabled=false`.
    pub fn interop_disabled(&self) -> SysResult<bool> {
        let out = self.wsl_raw(&["cat", crate::host::WSL_CONF], None)?;
        Ok(out.success() && parse_interop_disabled(&out.text()))
    }

    /// `networkingMode` in the user's `.wslconfig` under `home`, if the file sets one.
    pub fn wslconfig_network(&self, home: &Path) -> SysResult<Option<String>> {
        let text = self.local.read_file(&home.join(".wslconfig"))?;
        Ok(text.as_deref().and_then(parse_networking_mode))
    }

    /// The machine's portproxy rules (`netsh interface portproxy show v4tov4`).
    pub fn portproxy_rules(&self) -> SysResult<Vec<PortProxyRule>> {
        let inv = Invocation {
            program: tool_path(Tool::Netsh),
            args: ["interface", "portproxy", "show", "v4tov4"]
                .map(str::to_owned)
                .to_vec(),
            stdin: None,
        };
        let out = self.run("list portproxy rules", &inv)?;
        if !out.success() {
            return Err(cmd_error("list portproxy rules", &out));
        }
        Ok(relay::parse_portproxy_table(&out.text()))
    }

    /// The IPv4 addresses (with prefix lengths) of the WSL virtual adapter on this machine.
    pub fn wsl_adapter_addresses(&self) -> SysResult<Vec<AdapterAddr>> {
        let out = self.powershell(
            "query the WSL network adapter",
            &ps::wsl_adapter_addresses(),
        )?;
        Ok(relay::parse_adapter_addrs(&out.text()))
    }

    /// The distro's current IPv4 address: the first one `hostname -I` prints that is private,
    /// inside the WSL adapter's subnet and not the adapter's own (see [`relay::choose_wsl_ip`]).
    pub fn wsl_ip(&self) -> SysResult<std::net::Ipv4Addr> {
        let adapters = self.wsl_adapter_addresses()?;
        if adapters.is_empty() {
            return Err(SystemError::InvalidState(
                "the WSL virtual network adapter (vEthernet (WSL)) has no IPv4 address: this machine is probably in mirrored mode, where no relay is needed (use --network mirrored), or the distro has not started yet (start it once and retry)".to_owned(),
            ));
        }
        let out = self.wsl(&["hostname", "-I"])?;
        relay::choose_wsl_ip(&out.text(), &adapters).ok_or_else(|| {
            tracing::error!(output = %out.text(), ?adapters, "no hostname -I address is inside the WSL adapter subnet");
            SystemError::InvalidState(format!(
                "no address in the distro's `hostname -I` output {:?} is a private address inside the WSL adapter's subnet {adapters:?}",
                out.text()
            ))
        })
    }

    /// Run netsh with `args` and require success.
    fn netsh(&self, what: &str, args: Vec<String>) -> SysResult<Output> {
        let out = self.run(
            what,
            &Invocation {
                program: tool_path(Tool::Netsh),
                args,
                stdin: None,
            },
        )?;
        if out.success() {
            Ok(out)
        } else {
            Err(cmd_error(what, &out))
        }
    }

    /// Probe everything the plan depends on (`home` is where `.wslconfig` lives).
    pub fn probe(&self, home: &Path) -> SysResult<HostFacts> {
        let facts = HostFacts {
            hyperv_firewall: self.hyperv_firewall_available()?,
            sshd_ports: self.sshd_ports()?,
            authorized_keys: self.default_user_has_authorized_keys()?,
            windows_build: self.windows_build(),
            wslconfig_network: self.wslconfig_network(home)?,
            portproxy: self.portproxy_rules()?,
            admin_account: self.admin_account(),
            interop_disabled: self.interop_disabled()?,
        };
        tracing::info!(?facts, "probed host");
        Ok(facts)
    }

    /// Probe the native OpenSSH setup and the invoking account (read-only; needs an elevated
    /// token for the capability query).
    pub fn probe_native(&self) -> SysResult<(NativeFacts, KeyAccount)> {
        let out = self.powershell("probe the native OpenSSH setup", &ps::native_probe())?;
        let parsed = parse_native_probe(&out.text()).map_err(SystemError::InvalidState)?;
        tracing::info!(facts = ?parsed.0, account = ?parsed.1, "probed native host");
        Ok(parsed)
    }

    /// sshd's ed25519 host public key (the line in its `.pub` file), when it exists yet.
    pub fn host_public_key(&self, path: &Path) -> SysResult<Option<String>> {
        Ok(self
            .local
            .read_file(path)?
            .and_then(|t| t.lines().next().map(|l| l.trim().to_owned()))
            .filter(|l| !l.is_empty()))
    }

    /// TCP ports with a listener in the distro's network namespace.
    pub fn listening_ports(&self) -> SysResult<BTreeSet<u16>> {
        let out = self.wsl(&["ss", "-Hltn"])?;
        Ok(parse_listening_ports(&out.text()))
    }

    /// Make the running sshd match its configuration.
    ///
    /// Reloads systemd's view of the units, then restarts the socket (or the service when it is
    /// not socket-activated) if `port` is not yet listening, otherwise only reloads sshd.
    pub fn activate_sshd(&mut self, port: u16) -> SysResult<Activation> {
        tracing::info!(port, "activating sshd");
        // Never reload or restart into a configuration sshd would reject.
        self.wsl(&["/usr/sbin/sshd", "-t"])?;
        self.wsl(&["systemctl", "daemon-reload"])?;
        if self.listening_ports()?.contains(&port) {
            if self
                .wsl_raw(&["systemctl", "is-active", "--quiet", "ssh"], None)?
                .success()
            {
                self.wsl(&["systemctl", "reload", "ssh"])?;
            }
            return Ok(Activation::Reloaded);
        }
        self.restart_sshd()?;
        self.wait_listening(port, true)?;
        Ok(Activation::Restarted)
    }

    /// After removal: restart sshd when it still listens on `port` although it is no longer configured.
    pub fn deactivate_sshd(&mut self, port: u16) -> SysResult<Option<Activation>> {
        tracing::info!(port, "deactivating sshd");
        self.wsl(&["systemctl", "daemon-reload"])?;
        if !self.listening_ports()?.contains(&port) || self.sshd_ports()?.contains(&port) {
            return Ok(None);
        }
        self.restart_sshd()?;
        self.wait_listening(port, false)?;
        Ok(Some(Activation::Restarted))
    }

    fn restart_sshd(&self) -> SysResult<()> {
        let unit = if self
            .wsl_raw(&["systemctl", "is-enabled", "--quiet", "ssh.socket"], None)?
            .success()
        {
            "ssh.socket"
        } else {
            "ssh"
        };
        tracing::warn!(unit, "restarting sshd unit");
        self.wsl(&["systemctl", "restart", unit]).map(drop)
    }

    fn wait_listening(&self, port: u16, want: bool) -> SysResult<()> {
        for _ in 0..20 {
            if self.listening_ports()?.contains(&port) == want {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(SystemError::InvalidState(format!(
            "sshd {} listening on port {port} after restart",
            if want { "is not" } else { "is still" }
        )))
    }

    /// Start the keepalive task now (it otherwise starts at the next logon or boot).
    pub fn start_task(&self, name: &str) -> SysResult<()> {
        self.powershell("start scheduled task", &ps::task_start(name))
            .map(drop)
    }
}

/// Distro names from `wsl.exe -l -q` output: one per line, with the byte-order mark and any
/// line that is not a bare name (a help message) dropped.
pub fn parse_distro_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\u{fffd}'))
        .filter(|l| !l.is_empty() && !l.contains(char::is_whitespace))
        .map(str::to_owned)
        .collect()
}

/// Whether `wsl.exe` exists in the system directory.
pub fn wsl_exe_present() -> bool {
    std::path::Path::new(&tool_path(Tool::Wsl)).is_file()
}

/// Probe WSL on this laptop as the invoking user, changing nothing (`wsl_exe`: see [`wsl_exe_present`]).
pub fn probe_wsl(runner: &impl Runner, wsl_exe: bool) -> WslProbe {
    let program = tool_path(Tool::Wsl);
    let distros = if wsl_exe {
        let inv = Invocation {
            program,
            args: vec!["-l".into(), "-q".into()],
            stdin: None,
        };
        match runner.run(&inv) {
            Ok(out) if out.success() => Some(parse_distro_list(&out.text())),
            Ok(out) => {
                tracing::info!(code = ?out.code, "wsl --list failed; WSL is not set up");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not run wsl --list");
                None
            }
        }
    } else {
        None
    };
    let probe = WslProbe { wsl_exe, distros };
    tracing::info!(?probe, "probed WSL");
    probe
}

/// The last value of `key` in `[section]` of INI text (section and key names are not case
/// sensitive to WSL); `None` when the file does not set it.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim().eq_ignore_ascii_case(section);
        } else if in_section
            && let Some((k, value)) = line.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            found = Some(value.trim().to_owned());
        }
    }
    found
}

/// The `networkingMode` value in `[wsl2]` of `.wslconfig` text; `None` when the file does not set it.
pub fn parse_networking_mode(text: &str) -> Option<String> {
    ini_value(text, "wsl2", "networkingMode")
}

/// Whether `wsl.conf` text sets `[interop] enabled=false`.
pub fn parse_interop_disabled(text: &str) -> bool {
    ini_value(text, "interop", "enabled").is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

/// Ports from `sshd -T` output (`port 2222` lines).
pub fn parse_sshd_ports(text: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = text
        .lines()
        .filter_map(|l| l.strip_prefix("port ")?.trim().parse().ok())
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Listening TCP ports from `ss -Hltn` output (local address is the fourth column).
pub fn parse_listening_ports(text: &str) -> BTreeSet<u16> {
    text.lines()
        .filter_map(|l| {
            let local = l.split_whitespace().nth(3)?;
            local.rsplit_once(':')?.1.parse().ok()
        })
        .collect()
}

/// Parse the `key=value` lines of [`ps::native_probe`].
pub fn parse_native_probe(text: &str) -> Result<(NativeFacts, KeyAccount), String> {
    let get = |key: &str| {
        text.lines()
            .filter_map(|l| l.trim().split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.trim().to_owned())
    };
    let need = |key: &str| {
        get(key)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("the probe did not report {key}"))
    };
    let account = KeyAccount {
        name: need("account")?,
        sid: need("sid")?,
        admin: need("admin")? == "1",
        profile_dir: need("profile")?.into(),
    };
    account.validate()?;
    let service = match (get("service_start"), get("service_running")) {
        (Some(start), Some(running)) => StartType::parse(&start).map(|s| (s, running == "1")),
        _ => None,
    };
    let builtin_rule = match get("builtin_rule").as_deref() {
        Some("open") => BuiltinRule::Open,
        Some("scoped") => BuiltinRule::Scoped,
        _ => BuiltinRule::Absent,
    };
    let facts = NativeFacts {
        capability_installed: need("capability")? == "1",
        service,
        default_shell: get("default_shell").filter(|v| !v.is_empty()),
        builtin_rule,
    };
    Ok((facts, account))
}

/// A DACL in the form goway writes it: the auto-inheritance control flags (`AI`, `AR`) that
/// Windows adds when reading a descriptor back are dropped, so a read and a write compare equal.
pub fn normalize_sddl(sddl: &str) -> String {
    let Some(start) = sddl.find("D:") else {
        return sddl.to_owned();
    };
    let flags_start = start + 2;
    let flags_end = sddl[flags_start..]
        .find('(')
        .map_or(sddl.len(), |i| flags_start + i);
    let flags = sddl[flags_start..flags_end]
        .replace("AI", "")
        .replace("AR", "");
    format!("{}{flags}{}", &sddl[..flags_start], &sddl[flags_end..])
}

impl<R: Runner> System for HostSystem<R> {
    fn read_file(&self, path: &Path) -> SysResult<Option<String>> {
        if !is_wsl_path(path) {
            return crate::safefile::read_to_string(path).map_err(|e| local_io(path, e));
        }
        let p = Self::wsl_str(path)?;
        if !self.wsl_test("-e", p)? {
            return Ok(None);
        }
        let out = self.wsl(&["cat", "--", p])?;
        String::from_utf8(out.stdout)
            .map(Some)
            .map_err(|_| SystemError::InvalidState(format!("{p} is not valid UTF-8")))
    }

    fn write_file(&mut self, path: &Path, contents: &str) -> SysResult<()> {
        if !is_wsl_path(path) {
            tracing::debug!(path = %path.display(), "local: write file without following links");
            return crate::safefile::write(path, contents).map_err(|e| local_io(path, e));
        }
        let p = Self::wsl_str(path)?;
        tracing::debug!(path = p, bytes = contents.len(), "wsl: write file");
        let out = self.wsl_raw(&["tee", "--", p], Some(contents.as_bytes()))?;
        if out.success() {
            Ok(())
        } else {
            Err(cmd_error(&format!("write {p}"), &out))
        }
    }

    fn remove_file(&mut self, path: &Path) -> SysResult<()> {
        if !is_wsl_path(path) {
            return self.local.remove_file(path);
        }
        let p = Self::wsl_str(path)?;
        tracing::debug!(path = p, "wsl: remove file");
        self.wsl(&["rm", "-f", "--", p]).map(drop)
    }

    fn dir_exists(&self, path: &Path) -> SysResult<bool> {
        if !is_wsl_path(path) {
            return self.local.dir_exists(path);
        }
        self.wsl_test("-d", Self::wsl_str(path)?)
    }

    fn create_dir(&mut self, path: &Path) -> SysResult<()> {
        if !is_wsl_path(path) {
            return self.local.create_dir(path);
        }
        let p = Self::wsl_str(path)?;
        tracing::debug!(path = p, "wsl: create dir");
        let out = self.wsl_raw(&["mkdir", "--", p], None)?;
        if out.success() {
            Ok(())
        } else if out.error_text().contains("No such file") {
            Err(SystemError::NotFound(p.to_owned()))
        } else {
            Err(cmd_error(&format!("mkdir {p}"), &out))
        }
    }

    fn dir_is_empty(&self, path: &Path) -> SysResult<bool> {
        if !is_wsl_path(path) {
            return self.local.dir_is_empty(path);
        }
        let p = Self::wsl_str(path)?;
        if !self.wsl_test("-d", p)? {
            return Err(SystemError::NotFound(p.to_owned()));
        }
        Ok(self
            .wsl(&["find", p, "-mindepth", "1", "-maxdepth", "1"])?
            .text()
            .is_empty())
    }

    fn remove_dir(&mut self, path: &Path) -> SysResult<()> {
        if !is_wsl_path(path) {
            return self.local.remove_dir(path);
        }
        let p = Self::wsl_str(path)?;
        if !self.wsl_test("-d", p)? {
            return Ok(());
        }
        tracing::debug!(path = p, "wsl: remove dir");
        let out = self.wsl_raw(&["rmdir", "--", p], None)?;
        if out.success() {
            Ok(())
        } else if out.error_text().contains("not empty") {
            Err(SystemError::InvalidState(format!("{p} is not empty")))
        } else {
            Err(cmd_error(&format!("rmdir {p}"), &out))
        }
    }

    fn get_var(&self, name: &str) -> SysResult<Option<String>> {
        self.local.get_var(name)
    }

    fn set_var(&mut self, name: &str, value: &str) -> SysResult<()> {
        self.local.set_var(name, value)
    }

    fn remove_var(&mut self, name: &str) -> SysResult<()> {
        self.local.remove_var(name)
    }

    fn file_digest(&self, path: &Path) -> SysResult<Option<String>> {
        if !is_wsl_path(path) {
            return self.local.file_digest(path);
        }
        let p = Self::wsl_str(path)?;
        if !self.wsl_test("-f", p)? {
            return Ok(None);
        }
        let out = self.wsl(&["sha256sum", "--", p])?;
        Ok(out.text().split_whitespace().next().map(str::to_owned))
    }

    fn copy_file(&mut self, src: &Path, dest: &Path) -> SysResult<()> {
        match (is_wsl_path(src), is_wsl_path(dest)) {
            (false, false) => self.local.copy_file(src, dest),
            (true, true) => self
                .wsl(&["cp", "--", Self::wsl_str(src)?, Self::wsl_str(dest)?])
                .map(drop),
            _ => Err(SystemError::Unsupported(
                "copying between Windows and the distro",
            )),
        }
    }

    fn reg_key_exists(&self, key: &str) -> SysResult<bool> {
        self.local.reg_key_exists(key)
    }

    fn reg_key_create(&mut self, key: &str) -> SysResult<()> {
        self.local.reg_key_create(key)
    }

    fn reg_key_is_empty(&self, key: &str) -> SysResult<bool> {
        self.local.reg_key_is_empty(key)
    }

    fn reg_key_remove(&mut self, key: &str) -> SysResult<()> {
        self.local.reg_key_remove(key)
    }

    fn reg_get(&self, key: &str, name: &str) -> SysResult<Option<RegValue>> {
        self.local.reg_get(key, name)
    }

    fn reg_set(&mut self, key: &str, name: &str, value: &RegValue) -> SysResult<()> {
        self.local.reg_set(key, name, value)
    }

    fn reg_delete(&mut self, key: &str, name: &str) -> SysResult<()> {
        self.local.reg_delete(key, name)
    }

    fn get_mode(&self, path: &Path) -> SysResult<u32> {
        if !is_wsl_path(path) {
            return self.local.get_mode(path);
        }
        let p = Self::wsl_str(path)?;
        let out = self.wsl(&["stat", "-c", "%a", "--", p])?;
        u32::from_str_radix(&out.text(), 8).map_err(|_| {
            SystemError::InvalidState(format!("unreadable mode of {p}: {}", out.text()))
        })
    }

    fn set_mode(&mut self, path: &Path, mode: u32) -> SysResult<()> {
        if !is_wsl_path(path) {
            return self.local.set_mode(path, mode);
        }
        self.wsl(&["chmod", &format!("{mode:o}"), "--", Self::wsl_str(path)?])
            .map(drop)
    }

    fn get_acl(&self, path: &Path) -> SysResult<String> {
        if is_wsl_path(path) {
            return Err(SystemError::Unsupported("ACLs inside the distro"));
        }
        let p = path.display().to_string();
        let out = self.powershell("read a file's ACL", &ps::acl_get(&p))?;
        Ok(normalize_sddl(&out.text()))
    }

    fn set_acl(&mut self, path: &Path, sddl: &str) -> SysResult<()> {
        if is_wsl_path(path) {
            return Err(SystemError::Unsupported("ACLs inside the distro"));
        }
        let p = path.display().to_string();
        tracing::info!(path = %p, sddl, "setting a file's ACL");
        self.powershell("set a file's ACL", &ps::acl_set(&p, sddl))
            .map(drop)
    }

    fn resource_exists(&self, kind: ResourceKind, name: &str) -> SysResult<bool> {
        reject_wildcard(kind, name)?;
        match kind {
            ResourceKind::FirewallRule => {
                self.ps_flag("query firewall rule", &ps::firewall_exists(name))
            }
            ResourceKind::HyperVFirewallRule => {
                self.ps_flag("query Hyper-V firewall rule", &ps::hyperv_exists(name))
            }
            ResourceKind::ScheduledTask => {
                self.ps_flag("query scheduled task", &ps::task_exists(name))
            }
            ResourceKind::PortProxy => {
                let (addr, port) = Self::relay_name(name)?;
                // Only a rule shaped like the relay goway makes counts: one someone re-pointed
                // elsewhere since the install is theirs, not ours to report or delete.
                Ok(self
                    .portproxy_rules()?
                    .iter()
                    .any(|r| r.listen_address == addr && relay::is_goway_relay(r, port)))
            }
            ResourceKind::WslPackage => Ok(self.dpkg_state(name)? == DpkgState::Installed),
            ResourceKind::WslUnit => {
                // A unit that does not exist (an older distro without ssh.socket) has nothing
                // to enable, so it counts as satisfied and is never touched.
                let out = self.wsl_raw(&["systemctl", "is-enabled", name], None)?;
                Ok(match out.text().as_str() {
                    "enabled" | "enabled-runtime" | "static" | "alias" | "generated" => true,
                    _ => !self.unit_exists(name)?,
                })
            }
            ResourceKind::Service => {
                Self::sshd_spec(name)?;
                self.ps_flag("query sshd", &ps::sshd_exists())
            }
            ResourceKind::WindowsCapability => {
                Self::only_capability(name)?;
                self.ps_flag(
                    "query the OpenSSH Server capability",
                    &ps::capability_exists(name),
                )
            }
            ResourceKind::FirewallScope => {
                Self::only_builtin_rule(name)?;
                self.ps_flag(
                    "query the built-in firewall rule",
                    &ps::firewall_scope_exists(name),
                )
            }
            ResourceKind::SshKeyPair => Err(SystemError::Unsupported("ssh key pairs on the host")),
        }
    }

    fn resource_create(&mut self, kind: ResourceKind, name: &str, spec: &str) -> SysResult<()> {
        reject_wildcard(kind, name)?;
        tracing::info!(?kind, name, "creating resource");
        match kind {
            ResourceKind::FirewallRule => {
                let s: FirewallSpec = Self::spec(kind, name, spec)?;
                self.powershell("create firewall rule", &ps::firewall_create(name, &s))
                    .map(drop)
            }
            ResourceKind::HyperVFirewallRule => {
                let s: HyperVSpec = Self::spec(kind, name, spec)?;
                self.powershell("create Hyper-V firewall rule", &ps::hyperv_create(name, &s))
                    .map(drop)
            }
            ResourceKind::ScheduledTask => {
                // The relay refresh task's spec carries a script path; the keepalive's does not.
                if let Ok(s) = serde_json::from_str::<RelayTaskSpec>(spec) {
                    relay::check_script_path(&s.script).map_err(SystemError::InvalidState)?;
                    let script = ps::relay_task_create(
                        name,
                        &s,
                        &absolute_tool(Tool::Conhost)?,
                        &absolute_tool(Tool::Cmd)?,
                        &absolute_tool(Tool::PowerShell)?,
                    );
                    return self.powershell("register relay task", &script).map(drop);
                }
                let s: TaskSpec = Self::spec(kind, name, spec)?;
                let conhost = absolute_tool(Tool::Conhost)?;
                let wsl = absolute_tool(Tool::Wsl)?;
                self.powershell(
                    "register scheduled task",
                    &ps::task_create(name, &s, &conhost, &wsl),
                )
                .map(drop)
            }
            ResourceKind::PortProxy => {
                let s: PortProxySpec = Self::spec(kind, name, spec)?;
                let (addr, port) = Self::relay_name(name)?;
                if s.listen_address != addr.to_string() || s.port != port {
                    return Err(SystemError::InvalidState(format!(
                        "portproxy spec {spec} does not match its name {name}"
                    )));
                }
                let ip = self.wsl_ip()?;
                tracing::info!(%addr, port, %ip, "creating portproxy relay");
                self.netsh(
                    "create portproxy relay",
                    relay::netsh_args("add", &s.listen_address, port, &ip.to_string()),
                )
                .map(drop)
            }
            ResourceKind::WslPackage => {
                // `resource_exists` already refused every state but absent and clean.
                if self.dpkg_state(name)? != DpkgState::Absent {
                    return Err(SystemError::InvalidState(format!(
                        "package {name} is already installed; refusing to install over it"
                    )));
                }
                // Lists may be stale on a fresh distro; a failed update is not fatal by itself.
                let _ = self.wsl_raw(&["apt-get", "update"], None)?;
                self.wsl(&[
                    "env",
                    "DEBIAN_FRONTEND=noninteractive",
                    "apt-get",
                    "install",
                    "-y",
                    name,
                ])
                .map(drop)
            }
            ResourceKind::WslUnit => self.wsl(&["systemctl", "enable", name]).map(drop),
            ResourceKind::Service => {
                Self::sshd_spec(name)?;
                self.powershell("enable and start sshd", &ps::sshd_enable())
                    .map(drop)
            }
            ResourceKind::WindowsCapability => {
                Self::only_capability(name)?;
                let out = self.powershell(
                    "install the OpenSSH Server capability",
                    &ps::capability_add(name),
                )?;
                if out.text().contains("restart") {
                    tracing::warn!(
                        "Windows asks for a restart to finish the OpenSSH Server install"
                    );
                }
                Ok(())
            }
            ResourceKind::FirewallScope => {
                Self::only_builtin_rule(name)?;
                let s: ScopeSpec = Self::spec(kind, name, spec)?;
                self.powershell(
                    "limit the built-in firewall rule",
                    &ps::firewall_scope_set(name, &s.scope),
                )
                .map(drop)
            }
            ResourceKind::SshKeyPair => Err(SystemError::Unsupported("ssh key pairs on the host")),
        }
    }

    fn resource_delete(&mut self, kind: ResourceKind, name: &str) -> SysResult<()> {
        reject_wildcard(kind, name)?;
        tracing::info!(?kind, name, "deleting resource");
        match kind {
            ResourceKind::FirewallRule => self
                .powershell("remove firewall rule", &ps::firewall_delete(name))
                .map(drop),
            ResourceKind::HyperVFirewallRule => self
                .powershell("remove Hyper-V firewall rule", &ps::hyperv_delete(name))
                .map(drop),
            ResourceKind::ScheduledTask => self
                .powershell("remove scheduled task", &ps::task_delete(name))
                .map(drop),
            ResourceKind::PortProxy => {
                let (addr, port) = Self::relay_name(name)?;
                let rules = self.portproxy_rules()?;
                match rules
                    .iter()
                    .find(|r| r.listen_address == addr && r.listen_port == port)
                {
                    None => return Ok(()),
                    Some(rule) if !relay::is_goway_relay(rule, port) => {
                        tracing::warn!(
                            ?rule,
                            "the rule on the relay's address is not goway's; leaving it"
                        );
                        return Err(SystemError::InvalidState(format!(
                            "the portproxy rule {name} forwards to {}:{}, which is not the relay goway made; leaving it alone",
                            rule.connect_address, rule.connect_port
                        )));
                    }
                    Some(_) => {}
                }
                self.netsh(
                    "remove portproxy relay",
                    relay::netsh_delete_args(&addr.to_string(), port),
                )
                .map(drop)
            }
            ResourceKind::WslPackage => {
                // Remove, never purge: configuration, sshd_config and host keys stay.
                self.wsl(&[
                    "env",
                    "DEBIAN_FRONTEND=noninteractive",
                    "apt-get",
                    "remove",
                    "-y",
                    name,
                ])
                .map(drop)
            }
            ResourceKind::WslUnit => self.wsl(&["systemctl", "disable", name]).map(drop),
            ResourceKind::Service => {
                let spec = Self::sshd_spec(name)?;
                self.powershell("restore sshd", &ps::sshd_restore(&spec))
                    .map(drop)
            }
            ResourceKind::WindowsCapability => {
                Self::only_capability(name)?;
                self.powershell(
                    "remove the OpenSSH Server capability",
                    &ps::capability_remove(name),
                )
                .map(drop)
            }
            ResourceKind::FirewallScope => {
                Self::only_builtin_rule(name)?;
                self.powershell(
                    "restore the built-in firewall rule",
                    &ps::firewall_scope_restore(name),
                )
                .map(drop)
            }
            ResourceKind::SshKeyPair => Err(SystemError::Unsupported("ssh key pairs on the host")),
        }
    }
}

impl<R: Runner> HostSystem<R> {
    /// Classify a package by `dpkg-query` status. Only "absent" and "cleanly installed" are
    /// acted on; any other state (half-configured, removed with configuration left, unpacked,
    /// ...) is refused, so goway never installs over, or later removes, something it does not
    /// fully understand.
    fn dpkg_state(&self, name: &str) -> SysResult<DpkgState> {
        let out = self.wsl_raw(&["dpkg-query", "-W", "-f=${Status}", name], None)?;
        if !out.success() {
            return Ok(DpkgState::Absent);
        }
        let status = out.text();
        match status.as_str() {
            "install ok installed" => Ok(DpkgState::Installed),
            s if s.ends_with(" not-installed") => Ok(DpkgState::Absent),
            other => Err(SystemError::InvalidState(format!(
                "package {name} is in dpkg state {other:?}, not cleanly installed or absent; goway will not touch it. Fix it yourself first (for example `apt-get install -f`)"
            ))),
        }
    }

    /// Whether systemd knows a unit of this name.
    fn unit_exists(&self, name: &str) -> SysResult<bool> {
        let out = self.wsl_raw(&["systemctl", "cat", name], None)?;
        Ok(out.success())
    }
}
