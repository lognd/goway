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

/// Tell running programs the user environment changed (no-op off Windows).
#[cfg(not(windows))]
pub fn broadcast_environment_change() {
    tracing::debug!("no environment broadcast off Windows");
}

/// Start `command` so it survives this process and, where allowed, the session's job object.
///
/// Windows OpenSSH (and some terminals) put a session in a job that kills every descendant when
/// the session ends. `CREATE_BREAKAWAY_FROM_JOB` escapes it; if the job forbids that the spawn
/// is retried without the flag. Off Windows this is a plain spawn.
#[cfg(windows)]
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

/// Start `command` detached (plain spawn off Windows).
#[cfg(not(windows))]
pub fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    command.spawn()
}

/// Delete `exe` and then `dir` once this process has exited (it cannot delete its own image).
///
/// Spawns a hidden `cmd` that waits about two seconds, then removes the copy, its log and `dir`.
#[cfg(windows)]
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
    let mut command = Command::new("cmd");
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

/// Delete the relaunched copy after exit (no-op off Windows).
#[cfg(not(windows))]
pub fn schedule_self_delete(exe: &Path, _dir: &Path) {
    tracing::debug!(exe = %exe.display(), "no self-delete off Windows");
}
