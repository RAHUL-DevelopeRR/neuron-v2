//! Who may change a file on Windows, and where Windows keeps Program Files.
//!
//! - [`program_files_dir`] asks Windows for the Program Files folder
//!   (`FOLDERID_ProgramFiles`) instead of reading the `ProgramFiles` environment
//!   variable, which any process can set for its children.
//! - [`check_admin_only`] reads a file's or folder's owner and access control list
//!   and fails unless only SYSTEM, the Administrators group and `TrustedInstaller`
//!   own it and can change it.
//!
//! On non-Windows targets this crate exports nothing.

#![deny(unsafe_code)]

/// SIDs trusted to own and change machine-wide files: SYSTEM, the Administrators
/// group, and `NT SERVICE\TrustedInstaller` (which owns Program Files).
pub const ADMIN_SIDS: [&str; 3] = [
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// Access rights that let a principal change a file or a folder: write or append data
/// (for a folder: add a file or a subfolder), delete a child, delete it, change its
/// access control list or owner, and the generic write and all rights.
pub const CHANGE_RIGHTS: u32 = 0x0000_0002 // FILE_WRITE_DATA / FILE_ADD_FILE
    | 0x0000_0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
    | 0x0000_0040 // FILE_DELETE_CHILD
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE

#[cfg(windows)]
pub use imp::{check_admin_only, program_files_dir};

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::ptr::{addr_of, from_mut, null_mut};

    use windows_sys::core::PWSTR;
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE,
        ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_ALLOWED_OBJECT_ACE_TYPE,
    };
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

    use super::{ADMIN_SIDS, CHANGE_RIGHTS};

    /// Memory from `LocalAlloc`, freed on drop.
    struct LocalMemory(*mut c_void);

    impl Drop for LocalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a Win32 call that allocates with
                // `LocalAlloc`, and it is freed exactly once.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    /// The NUL-terminated wide string at `text`.
    ///
    /// # Safety
    /// `text` points to a valid NUL-terminated UTF-16 string.
    unsafe fn wide_to_string(text: *const u16) -> std::ffi::OsString {
        let mut length = 0;
        // SAFETY: the string is NUL-terminated, so every read up to the NUL is in it.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` units were just read from `text`.
        std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(text, length) })
    }

    /// The Program Files folder, as Windows reports it (not the environment).
    #[must_use]
    pub fn program_files_dir() -> Option<PathBuf> {
        let mut path: PWSTR = null_mut();
        // SAFETY: a valid folder id, no flags, the current user's token (null) and a
        // writable out-pointer.
        let result = unsafe {
            SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), from_mut(&mut path))
        };
        let found = (result >= 0 && !path.is_null())
            // SAFETY: on success `path` is a NUL-terminated wide string.
            .then(|| PathBuf::from(unsafe { wide_to_string(path) }));
        // SAFETY: the buffer is freed with `CoTaskMemFree` whether or not the call
        // succeeded, as the API asks; a null pointer is ignored.
        unsafe { CoTaskMemFree(path.cast_const().cast()) };
        found.filter(|path| path.is_absolute())
    }

    /// `S-1-5-18` for `sid`.
    ///
    /// # Safety
    /// `sid` points to a valid SID.
    unsafe fn sid_string(sid: PSID) -> Result<String, String> {
        let mut text: PWSTR = null_mut();
        // SAFETY: `sid` is valid (the caller's promise) and `text` is writable.
        if unsafe { ConvertSidToStringSidW(sid, from_mut(&mut text)) } == 0 {
            return Err(format!(
                "cannot read a SID: {}",
                std::io::Error::last_os_error()
            ));
        }
        let text = LocalMemory(text.cast());
        // SAFETY: on success `text` is a NUL-terminated wide string.
        Ok(unsafe { wide_to_string(text.0.cast()) }
            .to_string_lossy()
            .into_owned())
    }

    /// Fails unless only SYSTEM, the Administrators group and `TrustedInstaller` own
    /// `path` and hold rights that change it ([`CHANGE_RIGHTS`]); says why. Rights
    /// that only children inherit are not counted. A deny entry is not weighed: it
    /// could only make the check stricter than needed.
    pub fn check_admin_only(path: &Path) -> Result<(), String> {
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated and every out-pointer is writable; the
        // owner and list point into `descriptor`, which outlives their use below.
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                from_mut(&mut owner),
                null_mut(),
                from_mut(&mut dacl),
                null_mut(),
                from_mut(&mut descriptor),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "cannot read who may change {}: {}",
                path.display(),
                std::io::Error::from_raw_os_error(i32::try_from(status).unwrap_or(i32::MAX))
            ));
        }
        let _descriptor = LocalMemory(descriptor);
        if owner.is_null() {
            return Err(format!("{} has no owner", path.display()));
        }
        // SAFETY: a non-null owner points to a SID in `descriptor`.
        let owner = unsafe { sid_string(owner) }?;
        if !ADMIN_SIDS.contains(&owner.as_str()) {
            return Err(format!(
                "{} is owned by {owner}, not by an administrator",
                path.display()
            ));
        }
        if dacl.is_null() {
            return Err(format!(
                "{} has no access control list, so anyone can change it",
                path.display()
            ));
        }
        // SAFETY: plain-old-data struct; all-zero is a valid value.
        let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        let size = u32::try_from(size_of::<ACL_SIZE_INFORMATION>()).expect("struct size fits u32");
        // SAFETY: `dacl` is a valid list and `info` is writable, of the declared class
        // and size.
        let ok = unsafe {
            GetAclInformation(dacl, from_mut(&mut info).cast(), size, AclSizeInformation)
        };
        if ok == 0 {
            return Err(format!(
                "cannot read who may change {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        for index in 0..info.AceCount {
            let mut ace: *mut c_void = null_mut();
            // SAFETY: `index` is below the list's entry count.
            if unsafe { GetAce(dacl, index, from_mut(&mut ace)) } == 0 || ace.is_null() {
                return Err(format!(
                    "cannot read who may change {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: every entry starts with a header.
            let header = unsafe { *ace.cast::<ACE_HEADER>() };
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            match u32::from(header.AceType) {
                ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE => {
                    let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                    // SAFETY: an entry of these types has this layout.
                    let mask = unsafe { (*allowed).Mask };
                    if mask & CHANGE_RIGHTS == 0 {
                        continue;
                    }
                    // SAFETY: the SID starts at `SidStart` and lies within the entry.
                    let sid =
                        unsafe { sid_string(addr_of!((*allowed).SidStart).cast_mut().cast()) }?;
                    if !ADMIN_SIDS.contains(&sid.as_str()) {
                        return Err(format!("{sid} can change {}", path.display()));
                    }
                }
                ACCESS_ALLOWED_OBJECT_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_OBJECT_ACE_TYPE => {
                    return Err(format!(
                        "{} has an object access entry, which file permissions do not use",
                        path.display()
                    ));
                }
                // Deny and audit entries grant nothing.
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::Path;

    use super::{check_admin_only, program_files_dir};

    #[test]
    fn program_files_comes_from_windows_not_the_environment() {
        let dir = program_files_dir().expect("Windows reports Program Files");
        assert!(dir.is_absolute());
        assert!(dir.is_dir(), "{}", dir.display());
    }

    #[test]
    fn a_system_file_is_admin_only_and_a_temp_file_is_not() {
        // Owned by SYSTEM and TrustedInstaller; users may only read it.
        let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let hosts = Path::new(&windows).join(r"System32\drivers\etc\hosts");
        if hosts.is_file() {
            check_admin_only(&hosts).expect("the hosts file is admin-only");
            check_admin_only(hosts.parent().expect("etc")).expect("its folder is admin-only");
        }

        // The temp folder belongs to the user running the test, who can change it.
        let path = std::env::temp_dir().join(format!("winsec-test-{}", std::process::id()));
        std::fs::write(&path, "{}").expect("temp file");
        let refused = check_admin_only(&path).expect_err("a user's file is not admin-only");
        assert!(
            refused.contains("owned by") || refused.contains("can change"),
            "{refused}"
        );
        let _ = std::fs::remove_file(&path);
    }
}
