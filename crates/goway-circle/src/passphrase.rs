//! Passphrases for encrypted node keys: a redacting secret type, a no-echo tty prompt and the
//! askpass bridge that hands a passphrase to `ssh-keygen` without argv or files holding it.

use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::error::CryptoError;

/// The environment variable the askpass script reads; set only on the child process.
const ENV_VAR: &str = "GOWAY_CIRCLE_PASSPHRASE";

/// A secret passphrase that is wiped on drop and never printed.
#[derive(Clone)]
pub struct Passphrase(Zeroizing<String>);

impl Passphrase {
    /// Wrap `secret`; an empty passphrase is refused because it would mean no encryption.
    pub fn new(secret: String) -> Result<Self, CryptoError> {
        if secret.is_empty() {
            return Err(CryptoError::Tool(
                "an empty passphrase is not allowed".into(),
            ));
        }
        Ok(Self(Zeroizing::new(secret)))
    }

    /// The secret text, for handing to a child process environment only.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase(<redacted>)")
    }
}

/// Something that can produce a passphrase on demand (a terminal, a test double).
pub trait PassphraseSource {
    /// Ask for a passphrase, showing `prompt` to the user.
    fn passphrase(&self, prompt: &str) -> Result<Passphrase, CryptoError>;
}

/// Prompts on the controlling terminal (`/dev/tty`) with echo turned off.
#[derive(Debug, Clone, Copy, Default)]
pub struct TtyPrompt;

impl PassphraseSource for TtyPrompt {
    fn passphrase(&self, prompt: &str) -> Result<Passphrase, CryptoError> {
        let tty_in = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let mut tty_out = tty_in.try_clone()?;
        std::io::Write::write_all(&mut tty_out, prompt.as_bytes())?;
        std::io::Write::flush(&mut tty_out)?;
        let stty = |arg: &str| {
            Command::new("stty")
                .arg(arg)
                .stdin(Stdio::from(tty_in.try_clone().map_err(CryptoError::Io)?))
                .status()
                .map_err(|e| CryptoError::Tool(format!("cannot run stty: {e}")))
        };
        if !stty("-echo")?.success() {
            warn!("could not turn off terminal echo; refusing to read a passphrase");
            return Err(CryptoError::Tool("cannot disable terminal echo".into()));
        }
        let mut line = Zeroizing::new(String::new());
        let read = BufReader::new(&tty_in).read_line(&mut line);
        let _ = stty("echo");
        let _ = std::io::Write::write_all(&mut tty_out, b"\n");
        read?;
        let secret = line.trim_end_matches(['\r', '\n']).to_owned();
        debug!("read a passphrase from the terminal");
        Passphrase::new(secret)
    }
}

/// Ask `source` for a new passphrase twice and refuse unless both entries match.
// frob:ticket 01M54X1N9MVWMYR7B0A2FFPC53
pub fn ask_new_passphrase(
    source: &dyn PassphraseSource,
    prompt: &str,
    confirm_prompt: &str,
) -> Result<Passphrase, CryptoError> {
    let first = source.passphrase(prompt)?;
    let second = source.passphrase(confirm_prompt)?;
    if first.expose() != second.expose() {
        warn!("new passphrase refused: the confirmation did not match");
        return Err(CryptoError::Mismatch);
    }
    debug!("new passphrase confirmed");
    Ok(first)
}

/// A scripted source that returns its passphrases in order, for tests driving a double prompt.
#[derive(Debug, Default)]
pub struct ScriptedPassphrase(std::sync::Mutex<std::collections::VecDeque<Passphrase>>);

impl ScriptedPassphrase {
    /// A source that answers each prompt with the next of `answers`.
    pub fn new(answers: Vec<Passphrase>) -> Self {
        Self(std::sync::Mutex::new(answers.into()))
    }
}

impl PassphraseSource for ScriptedPassphrase {
    fn passphrase(&self, _prompt: &str) -> Result<Passphrase, CryptoError> {
        self.0
            .lock()
            .expect("scripted passphrase lock")
            .pop_front()
            .ok_or_else(|| CryptoError::Tool("no scripted passphrase left".into()))
    }
}

// frob:tests crates/goway-circle/src/passphrase.rs::PassphraseSource.passphrase
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_source_returns_its_passphrase() {
        let p = Passphrase::new("abc def ghi".into()).unwrap();
        let got = FixedPassphrase(p).passphrase("x: ").unwrap();
        assert_eq!(got.expose(), "abc def ghi");
    }

    fn pp(s: &str) -> Passphrase {
        Passphrase::new(s.into()).unwrap()
    }

    // frob:tests crates/goway-circle/src/passphrase.rs::ask_new_passphrase
    #[test]
    fn passphrase_confirmation_mismatch_is_refused() {
        let src = ScriptedPassphrase::new(vec![pp("one one one"), pp("two two two")]);
        let err = ask_new_passphrase(&src, "a: ", "b: ").unwrap_err();
        assert!(matches!(err, CryptoError::Mismatch));
    }

    // frob:tests crates/goway-circle/src/passphrase.rs::ask_new_passphrase
    #[test]
    fn passphrase_confirmation_match_is_accepted() {
        let src = ScriptedPassphrase::new(vec![pp("same same"), pp("same same")]);
        let got = ask_new_passphrase(&src, "a: ", "b: ").unwrap();
        assert_eq!(got.expose(), "same same");
    }
}

/// A fixed passphrase, for tests and non-interactive callers that already hold one.
#[derive(Debug, Clone)]
pub struct FixedPassphrase(pub Passphrase);

impl PassphraseSource for FixedPassphrase {
    fn passphrase(&self, _prompt: &str) -> Result<Passphrase, CryptoError> {
        Ok(self.0.clone())
    }
}

/// A scratch askpass script; keep it alive while the child runs, it is deleted on drop.
pub(crate) struct Askpass {
    dir: tempfile::TempDir,
}

impl Askpass {
    /// Write the 0700 helper script that prints the passphrase from the environment.
    pub(crate) fn new() -> Result<Self, CryptoError> {
        let dir = tempfile::tempdir()?;
        let script = dir.path().join("askpass.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"${ENV_VAR}\"\n"),
        )?;
        make_executable(&script)?;
        Ok(Self { dir })
    }

    /// Point `cmd` at the script and give it `pass` through its own environment only.
    pub(crate) fn apply(&self, cmd: &mut Command, pass: &Passphrase) {
        cmd.env("SSH_ASKPASS", self.dir.path().join("askpass.sh"))
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(
                "DISPLAY",
                std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
            )
            .env(ENV_VAR, pass.expose())
            .stdin(Stdio::null());
        debug!("passphrase handed to ssh-keygen through askpass");
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), CryptoError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), CryptoError> {
    Ok(())
}

/// Return true when the OpenSSH private key at `path` is passphrase-encrypted.
///
/// Reads the key header (cipher name) only; a missing or non-OpenSSH file is `false`.
pub fn key_is_encrypted(path: &Path) -> bool {
    use base64::Engine as _;
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(body.trim()) else {
        return false;
    };
    let magic = b"openssh-key-v1\0";
    let Some(rest) = blob.strip_prefix(magic.as_slice()) else {
        return false;
    };
    let Some((len, rest)) = rest.split_first_chunk::<4>() else {
        return false;
    };
    let len = u32::from_be_bytes(*len) as usize;
    let encrypted = rest.get(..len).is_some_and(|c| c != b"none");
    info!(key = %path.display(), encrypted, "checked key encryption");
    encrypted
}

/// Generate an encrypted ed25519 key at `private` using `pass`.
pub(crate) fn keygen_encrypted(private: &Path, pass: &Passphrase) -> Result<(), CryptoError> {
    let askpass = Askpass::new()?;
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-t", "ed25519", "-C", "", "-f"])
        .arg(private);
    askpass.apply(&mut cmd, pass);
    let out = cmd
        .output()
        .map_err(|e| CryptoError::Tool(format!("cannot run ssh-keygen: {e}")))?;
    if !out.status.success() {
        warn!(status = ?out.status, "ssh-keygen could not make an encrypted key");
        return Err(CryptoError::Tool(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}
