//! Thin, audited Windows API wrappers the elevated code relies on instead of environment
//! variables and search paths a non-administrator process can influence.
//!
//! Everything here asks the operating system (known folders, the system directory, the token,
//! the security descriptor) and never the process environment. Off Windows the functions that
//! cannot exist return an error, and the path helpers fall back to plain names so the pure logic
//! stays testable.

use std::path::PathBuf;

/// A system tool goway-setup runs, always by absolute path on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// `wsl.exe`.
    Wsl,
    /// Windows PowerShell 5.1.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
    /// `netsh.exe`, which manages the portproxy relay.
    Netsh,
    /// `conhost.exe`, which runs a task's program without a window.
    Conhost,
}

impl Tool {
    /// The tool's path relative to `%SystemRoot%\System32`.
    fn relative(self) -> &'static str {
        match self {
            Self::Wsl => "wsl.exe",
            Self::PowerShell => r"WindowsPowerShell\v1.0\powershell.exe",
            Self::Cmd => "cmd.exe",
            Self::Netsh => "netsh.exe",
            Self::Conhost => "conhost.exe",
        }
    }

    /// The bare file name (what a non-Windows build and its tests see).
    fn file_name(self) -> &'static str {
        match self {
            Self::Wsl => "wsl.exe",
            Self::PowerShell => "powershell.exe",
            Self::Cmd => "cmd.exe",
            Self::Netsh => "netsh.exe",
            Self::Conhost => "conhost.exe",
        }
    }
}

/// Where to find `tool`: `<system directory>\System32\...` on Windows, the bare name elsewhere.
///
/// A bare name would be resolved through the application directory and `PATH`, both writable
/// by an ordinary user, so an elevated process must never use one.
pub fn tool_path(tool: Tool) -> String {
    #[cfg(windows)]
    {
        match windows_dir() {
            Ok(dir) => dir
                .join("System32")
                .join(tool.relative())
                .display()
                .to_string(),
            Err(e) => {
                tracing::error!(error = %e, "cannot locate the Windows directory; using the bare tool name");
                tool.file_name().to_owned()
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = Tool::relative;
        tool.file_name().to_owned()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // GetSystemWindowsDirectoryW FFI; see SAFETY
/// `%SystemRoot%` as the system reports it (not from the environment).
pub fn windows_dir() -> std::io::Result<PathBuf> {
    use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    let mut buf = [0u16; 260];
    // SAFETY: `buf` is writable for the 260 UTF-16 units passed as its length.
    let n = unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), 260) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

#[cfg(windows)]
#[allow(unsafe_code)] // SHGetKnownFolderPath FFI; see SAFETY
/// The machine-wide `ProgramData` directory from the known-folder API.
pub fn program_data_dir() -> std::io::Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};
    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: the folder id is a valid static GUID; `raw` is a valid out-pointer. On success the
    // API returns a NUL-terminated string that is copied out and then freed exactly once.
    unsafe {
        let hr = SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, std::ptr::null_mut(), &raw mut raw);
        if hr < 0 || raw.is_null() {
            return Err(std::io::Error::other(format!(
                "SHGetKnownFolderPath failed: {hr:#x}"
            )));
        }
        let text = wide_to_string(raw);
        CoTaskMemFree(raw.cast());
        Ok(PathBuf::from(text))
    }
}

#[cfg(not(windows))]
/// The machine-wide data directory (a stand-in off Windows: `PROGRAMDATA` or `/var/lib`).
pub fn program_data_dir() -> std::io::Result<PathBuf> {
    Ok(std::env::var_os("PROGRAMDATA").map_or_else(|| PathBuf::from("/var/lib"), PathBuf::from))
}

#[cfg(windows)]
#[allow(unsafe_code)]
/// Copy a NUL-terminated UTF-16 string.
///
/// # Safety
/// `p` must point to a valid NUL-terminated UTF-16 string.
unsafe fn wide_to_string(p: *const u16) -> String {
    let mut len = 0;
    // SAFETY: the caller guarantees a terminating NUL, so every read below stays in bounds.
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

#[cfg(windows)]
/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
#[allow(unsafe_code)] // token and SID FFI; see SAFETY
/// The SID of the user this process runs as, in `S-1-5-21-...` form.
pub fn current_user_sid() -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: out-pointers are valid; the buffer is 8-byte aligned (Vec<u64>) and at least as
    // large as the size the first call reported; the token handle is closed once; the string
    // returned by ConvertSidToStringSidW is copied and released with LocalFree once.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut needed = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &raw mut needed);
        let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        );
        CloseHandle(token);
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &raw mut text) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sid = wide_to_string(text);
        LocalFree(text.cast());
        Ok(sid)
    }
}

#[cfg(not(windows))]
/// The SID of the user (unavailable off Windows).
pub fn current_user_sid() -> std::io::Result<String> {
    Err(std::io::Error::other("user SIDs only exist on Windows"))
}

#[cfg(windows)]
#[allow(unsafe_code)] // security descriptor FFI; see SAFETY
/// The owner and DACL of `path` as an SDDL string (`O:...D:...`).
pub fn owner_and_dacl_sddl(path: &std::path::Path) -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION};
    let name = wide(&path.display().to_string());
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; the out-pointers are valid; the descriptor allocated by
    // GetNamedSecurityInfoW and the string allocated by the conversion are each LocalFree'd once
    // after use, and the string is copied out first.
    unsafe {
        let status = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut descriptor,
        );
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status.cast_signed()));
        }
        let mut text: *mut u16 = std::ptr::null_mut();
        let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut text,
            std::ptr::null_mut(),
        );
        let result = if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(wide_to_string(text))
        };
        if !text.is_null() {
            LocalFree(text.cast());
        }
        LocalFree(descriptor);
        result
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // security descriptor FFI; see SAFETY
/// Create the directory `path` carrying `sddl` from the start (no window where it inherits a
/// wider ACL); an existing directory is left alone and reported as `AlreadyExists`.
pub fn create_dir_with_sddl(path: &std::path::Path, sddl: &str) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let name = wide(&path.display().to_string());
    let sddl = wide(sddl);
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated and outlive the calls; `attributes` points at the
    // descriptor the conversion allocated, which is freed once after CreateDirectoryW returns.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let ok = CreateDirectoryW(name.as_ptr(), &raw const attributes);
        let result = if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        };
        LocalFree(descriptor);
        result
    }
}
