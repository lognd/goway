//! Administrator detection and UAC relaunch for the host component (firewall rules need it).
//!
//! Over Windows OpenSSH an administrator account already holds a full (High integrity) token, so
//! [`is_elevated`] is true and nothing is relaunched. In an ordinary non-elevated console the
//! installer re-runs only its host component through the `runas` verb (a UAC prompt) and waits
//! for the exit code. The elevated exe and its arguments are handed to `ShellExecuteExW`
//! directly: no `cmd.exe` is involved, so no shell metacharacter in an argument can mean
//! anything. The elevated run writes its own output to a log in the administrator-only state
//! directory (see [`crate::admin`]), which the caller prints afterwards.

use std::path::Path;

/// Quote one argument for a Windows command line as `CommandLineToArgvW` and the C runtime
/// parse it: backslashes are literal except in front of a quote.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// The parameter string that passes `args` to the elevated exe (no program name, no shell).
pub fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether Windows could show a UAC prompt: only interactive desktop sessions set `SESSIONNAME`
/// (`Console`, `RDP-Tcp#n`); OpenSSH and service sessions do not, and a prompt there would hang.
pub fn can_prompt() -> bool {
    std::env::var_os("SESSIONNAME").is_some_and(|v| !v.is_empty())
}

#[cfg(windows)]
#[allow(unsafe_code)] // token query FFI; see SAFETY
/// Whether this process holds an elevated (administrator) token.
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

#[cfg(windows)]
#[allow(unsafe_code)] // ShellExecuteExW FFI; see SAFETY
/// Run `exe parameters` elevated (UAC prompt), wait, and return its exit code.
pub fn run_elevated(exe: &Path, parameters: &str) -> std::io::Result<u32> {
    use crate::sysapi::{wide, windows_dir};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let verb = wide("runas");
    let file = wide(&exe.display().to_string());
    let params = wide(parameters);
    let directory = wide(&windows_dir()?.join("System32").display().to_string());
    tracing::info!(exe = %exe.display(), parameters, "relaunching elevated through UAC");
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
        info.lpDirectory = directory.as_ptr();
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
pub fn run_elevated(_exe: &Path, _parameters: &str) -> std::io::Result<u32> {
    Err(std::io::Error::other(
        "elevation is only available on Windows",
    ))
}

/// Point this process's standard output and error at `log` (the elevated run has no console the
/// caller can read); the file stays open for the life of the process.
#[cfg(windows)]
#[allow(unsafe_code)] // SetStdHandle FFI; see SAFETY
pub fn redirect_output(log: std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let handle = log.as_raw_handle();
    // SAFETY: `handle` is a valid open file handle; it is leaked below so it outlives every use.
    let ok = unsafe {
        SetStdHandle(STD_OUTPUT_HANDLE, handle) != 0 && SetStdHandle(STD_ERROR_HANDLE, handle) != 0
    };
    std::mem::forget(log);
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Redirect output to the log (unsupported off Windows, where nothing is ever relaunched).
#[cfg(not(windows))]
pub fn redirect_output(_log: std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::other("elevated runs only exist on Windows"))
}
