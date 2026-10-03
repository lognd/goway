//! Administrator detection and UAC relaunch for the host component (firewall rules need it).
//!
//! Over Windows OpenSSH an administrator account already holds a full (High integrity) token, so
//! [`is_elevated`] is true and nothing is relaunched. In an ordinary non-elevated console the
//! installer re-runs itself through the `runas` verb (a UAC prompt), with its output captured in
//! a log file that the caller prints afterwards, and waits for the exit code.

use std::path::Path;

/// Quote one argument for a Windows command line (`cmd.exe` and the C runtime rules).
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

/// The `cmd.exe` parameters that run `exe args...` with all output written to `log`.
///
/// `/S` strips the outermost quote pair, so the inner quoting survives; the exit code of `cmd`
/// is that of the program.
pub fn elevated_parameters(exe: &Path, args: &[String], log: &Path) -> String {
    let mut line = quote_arg(&exe.display().to_string());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    format!(
        "/D /S /C \"{line} > {} 2>&1\"",
        quote_arg(&log.display().to_string())
    )
}

/// Whether Windows could show a UAC prompt: only interactive desktop sessions set `SESSIONNAME`
/// (`Console`, `RDP-Tcp#n`); OpenSSH and service sessions do not, and a prompt there would hang.
pub fn can_prompt() -> bool {
    std::env::var_os("SESSIONNAME").is_some_and(|v| !v.is_empty())
}

/// Whether this process holds an elevated (administrator) token.
#[cfg(windows)]
#[allow(unsafe_code)] // token query FFI; see SAFETY
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: `token` is a valid out-pointer; the pseudo-handle from GetCurrentProcess needs no
    // closing; `elevation` is a correctly sized, writable TOKEN_ELEVATION; the opened token
    // handle is closed exactly once.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) == 0 {
            tracing::warn!("OpenProcessToken failed; assuming not elevated");
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(0),
            &raw mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Whether this process holds an elevated token (nothing to elevate off Windows).
#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    true
}

/// Run `cmd.exe <parameters>` elevated (UAC prompt), wait, and return its exit code.
#[cfg(windows)]
#[allow(unsafe_code)] // ShellExecuteExW FFI; see SAFETY
pub fn run_elevated(parameters: &str) -> std::io::Result<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let verb = wide("runas");
    let file = wide("cmd.exe");
    let params = wide(parameters);
    tracing::info!(parameters, "relaunching elevated through UAC");
    // SAFETY: SHELLEXECUTEINFOW is plain data, valid when zeroed then filled; the wide strings
    // are NUL-terminated and outlive the call; the process handle returned thanks to
    // SEE_MASK_NOCLOSEPROCESS is waited on, queried and closed exactly once.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0);
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&raw mut info) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let ok = GetExitCodeProcess(info.hProcess, &raw mut code);
        CloseHandle(info.hProcess);
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(code)
    }
}

/// Run elevated (unsupported off Windows).
#[cfg(not(windows))]
pub fn run_elevated(_parameters: &str) -> std::io::Result<u32> {
    Err(std::io::Error::other(
        "elevation is only available on Windows",
    ))
}
