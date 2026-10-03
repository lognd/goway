//! Windows-only side effects that are not journaled state: the environment-change broadcast
//! and the self-delete helper. Everything here is a no-op (logged) on other platforms.

use std::path::Path;

#[cfg(windows)]
#[allow(unsafe_code)] // one FFI call with a NUL-terminated static string; see SAFETY
/// Tell running programs (Explorer, new consoles) that the user environment changed.
///
/// Broadcasts `WM_SETTINGCHANGE` with the string "Environment" so new shells see the new Path.
/// Best effort: a timeout or failure is logged, never fatal.
pub fn broadcast_environment_change() {
    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let environment: Vec<u16> = "Environment\0".encode_utf16().collect();
    let mut result = 0usize;
    // SAFETY: `environment` is NUL-terminated and outlives the call; `result` is a valid
    // out-pointer; HWND_BROADCAST with a string lparam is the documented use of this message.
    let ok = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            &raw mut result,
        )
    };
    if ok == 0 {
        tracing::warn!("WM_SETTINGCHANGE broadcast failed or timed out");
    } else {
        tracing::info!("broadcast WM_SETTINGCHANGE Environment");
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // one FFI call with a constant flag; see SAFETY
/// Limit DLL loading to the system directory for the rest of the process's life.
///
/// Without it a DLL planted beside the exe (a user-writable directory for the installed copy)
/// can be loaded into an elevated process. Best effort; failure is logged.
pub fn restrict_dll_search() {
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
    };
    // SAFETY: the flag is a documented constant and the call takes no pointers.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        tracing::warn!(error = %std::io::Error::last_os_error(), "could not restrict the DLL search path");
    }
}

#[cfg(not(windows))]
/// Limit DLL loading to the system directory (no-op off Windows).
pub fn restrict_dll_search() {}

#[cfg(not(windows))]
/// Tell running programs the user environment changed (no-op off Windows).
pub fn broadcast_environment_change() {
    tracing::debug!("no environment broadcast off Windows");
}

#[cfg(windows)]
/// Start `command` so it survives this process and, where allowed, the session's job object.
///
/// Windows OpenSSH (and some terminals) put a session in a job that kills every descendant when
/// the session ends. `CREATE_BREAKAWAY_FROM_JOB` escapes it; if the job forbids that the spawn
/// is retried without the flag. Off Windows this is a plain spawn.
pub fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    match command.creation_flags(CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(child) => Ok(child),
        Err(e) => {
            tracing::warn!(error = %e, "job breakaway refused; spawning inside the job");
            command.creation_flags(0).spawn()
        }
    }
}

#[cfg(not(windows))]
/// Start `command` detached (plain spawn off Windows).
pub fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    command.spawn()
}

#[cfg(windows)]
/// Delete `exe` and then `dir` once this process has exited (it cannot delete its own image).
///
/// Spawns a hidden `cmd` that waits about two seconds, then removes the copy, its log and `dir`.
pub fn schedule_self_delete(exe: &Path, dir: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let script = format!(
        "/C ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & del /f /q \"{}\\uninstall.log\" & rmdir \"{}\"",
        exe.display(),
        dir.display(),
        dir.display()
    );
    let mut command = Command::new(crate::sysapi::tool_path(crate::sysapi::Tool::Cmd));
    command
        .raw_arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let spawned = command
        .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| command.creation_flags(CREATE_NO_WINDOW).spawn());
    match spawned {
        Ok(_) => tracing::info!(exe = %exe.display(), "scheduled deletion of the relaunched copy"),
        Err(e) => tracing::warn!(exe = %exe.display(), error = %e, "could not schedule deletion"),
    }
}

#[cfg(not(windows))]
/// Delete the relaunched copy after exit (no-op off Windows).
pub fn schedule_self_delete(exe: &Path, _dir: &Path) {
    tracing::debug!(exe = %exe.display(), "no self-delete off Windows");
}

#[cfg(windows)]
/// Remove `dir` and its contents, then `root` if empty, once this process has exited.
///
/// Used for the administrator-only state directory when it still holds this process's own exe
/// or log. Spawns a hidden `cmd` (absolute path) that waits about five seconds, which also
/// leaves the caller time to read the elevated run's log; it inherits this process's token.
pub fn schedule_dir_removal(dir: &Path, root: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let script = format!(
        "/C ping -n 6 127.0.0.1 >nul & rmdir /s /q \"{}\" & rmdir \"{}\"",
        dir.display(),
        root.display()
    );
    let mut command = Command::new(crate::sysapi::tool_path(crate::sysapi::Tool::Cmd));
    command
        .raw_arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let spawned = command
        .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| command.creation_flags(CREATE_NO_WINDOW).spawn());
    match spawned {
        Ok(_) => tracing::info!(dir = %dir.display(), "scheduled removal of the state directory"),
        Err(e) => tracing::warn!(dir = %dir.display(), error = %e, "could not schedule removal"),
    }
}

#[cfg(not(windows))]
/// Remove the state directory after exit (no-op off Windows).
pub fn schedule_dir_removal(dir: &Path, _root: &Path) {
    tracing::debug!(dir = %dir.display(), "no scheduled removal off Windows");
}
