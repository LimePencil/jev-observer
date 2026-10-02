//! Windows token files are created with a protected, current-user DACL before
//! any secret is written. Validation uses the opened handle, never a second path.
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{null, null_mut},
};

use anyhow::{Context, Result, bail};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        GetFileInformationByHandle, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn sid_text(sid: *mut c_void) -> io::Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _allocation = LocalAllocation(text.cast());
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
            text, length,
        )))
    }
}

fn descriptor(inherit: bool) -> io::Result<LocalAllocation> {
    let mut handle = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(handle.as_raw_handle(), TokenUser, null_mut(), 0, &mut bytes);
    }
    if bytes == 0 {
        return Err(io::Error::last_os_error());
    }
    // usize storage ensures TOKEN_USER has proper pointer alignment.
    let mut buffer = vec![0_usize; (bytes as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            handle.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = sid_text(user.User.Sid)?;
    let flags = if inherit { "OICI" } else { "" };
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;{flags};FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut output = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut output,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(LocalAllocation(output))
}

fn validate(handle: HANDLE, directory: bool) -> Result<()> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(io::Error::last_os_error()).context("Inspect private file");
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        bail!("Private storage must be a regular file or directory, not a reparse point");
    }
    let expected = descriptor(false)?;
    let mut expected_owner = null_mut();
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorOwner(expected.0, &mut expected_owner, &mut defaulted) } == 0 {
        return Err(io::Error::last_os_error()).context("Read current user SID");
    }
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut security: PSECURITY_DESCRIPTOR = null_mut();
    let code = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut security,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32))
            .context("Read private storage permissions");
    }
    let _security = LocalAllocation(security);
    let trusted = |sid| -> Result<bool> {
        Ok(unsafe { EqualSid(sid, expected_owner) } != 0
            || (directory && matches!(sid_text(sid)?.as_str(), "S-1-5-18" | "S-1-5-32-544")))
    };
    if owner.is_null() || !trusted(owner)? {
        bail!("Private storage must belong to the current user");
    }
    if acl.is_null() || unsafe { (*acl).AceCount } == 0 {
        bail!("Private storage must have an explicit restricted ACL");
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(security, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error()).context("Read ACL inheritance");
    }
    if !directory && control & SE_DACL_PROTECTED == 0 {
        bail!("Dashboard access token must not inherit directory permissions");
    }
    for index in 0..unsafe { (*acl).AceCount } as u32 {
        let mut entry = null_mut();
        if unsafe { GetAce(acl, index, &mut entry) } == 0 {
            return Err(io::Error::last_os_error()).context("Read private storage ACL");
        }
        let header = unsafe { &*entry.cast::<ACE_HEADER>() };
        // ACCESS_ALLOWED_ACE_TYPE is 0. Reject unrecognized and conditional ACEs.
        if header.AceType != 0 {
            bail!("Unsupported private storage ACL entry");
        }
        let allowed = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = (&allowed.SidStart as *const u32).cast_mut().cast();
        if !trusted(sid)? {
            bail!("Private storage grants access to another account");
        }
    }
    Ok(())
}

pub fn create_private_file(path: &Path) -> io::Result<File> {
    let security = descriptor(false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate(file.as_raw_handle(), false).map_err(io::Error::other)?;
    Ok(file)
}

pub fn open_private_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    validate(file.as_raw_handle(), false)?;
    Ok(file)
}

pub fn prepare_database_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let default_workspace = parent
        .file_name()
        .is_some_and(|name| name == ".jev-observer");
    let rights = READ_CONTROL
        | if default_workspace {
            WRITE_DAC | WRITE_OWNER
        } else {
            0
        };
    let directory = OpenOptions::new()
        .access_mode(rights)
        .share_mode(3)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(parent)?;
    if default_workspace {
        let information = directory.metadata()?;
        use std::os::windows::fs::MetadataExt;
        if information.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!("Observer workspace must not be a reparse point");
        }
        let security = descriptor(true)?;
        let mut owner = null_mut();
        let mut defaulted = 0;
        let mut present = 0;
        let mut acl = null_mut();
        if unsafe { GetSecurityDescriptorOwner(security.0, &mut owner, &mut defaulted) } == 0
            || unsafe {
                GetSecurityDescriptorDacl(security.0, &mut present, &mut acl, &mut defaulted)
            } == 0
        {
            return Err(io::Error::last_os_error()).context("Prepare workspace ACL");
        }
        let code = unsafe {
            SetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                owner,
                null_mut(),
                acl,
                null(),
            )
        };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32))
                .context("Restrict Observer workspace");
        }
    }
    validate(directory.as_raw_handle(), true)
        .context("Use a private directory for --db; other ordinary accounts must not have access")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn private_token_is_reused_and_broad_acl_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("observer.sqlite");
        let (first, path) = crate::access::Access::open(&database).unwrap();
        let (second, _) = crate::access::Access::open(&database).unwrap();
        assert!(second.allows_token(Some(first.token())));
        let status = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/grant", "*S-1-1-0:(R)"])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(crate::access::Access::open(&database).is_err());
    }

    #[test]
    fn private_file_is_created_exclusively_and_directories_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let token = directory.path().join("token");
        {
            let mut file = create_private_file(&token).unwrap();
            file.write_all(b"example").unwrap();
        }
        assert_eq!(
            create_private_file(&token).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let mut content = String::new();
        open_private_file(&token)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "example");
        assert!(open_private_file(directory.path()).is_err());
    }
}
