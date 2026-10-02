#[cfg(unix)]
use std::fs;
#[cfg(any(unix, windows))]
use std::io::{Read, Write};
use std::path::Path;

#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};

const PREFIX: &str = "jo_access_";

#[derive(Clone)]
pub struct Access {
    token: String,
    basic: String,
}

fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

impl Access {
    fn new(token: String) -> Result<Self> {
        if !token.starts_with(PREFIX)
            || token.len() != PREFIX.len() + 64
            || !token[PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Dashboard access token file is invalid");
        }
        let basic = format!("Basic {}", STANDARD.encode(format!("observer:{token}")));
        Ok(Self { token, basic })
    }

    #[cfg(not(any(unix, windows)))]
    pub fn open(_database_path: &Path) -> Result<(Self, std::path::PathBuf)> {
        bail!("Dashboard access token permissions cannot be verified on this platform");
    }

    #[cfg(windows)]
    pub fn open(database_path: &Path) -> Result<(Self, std::path::PathBuf)> {
        let path = database_path.with_extension("access-token");
        let token = match crate::windows_private::create_private_file(&path) {
            Ok(mut file) => {
                let token = format!(
                    "{PREFIX}{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                file.write_all(token.as_bytes())?;
                file.sync_all()?;
                token
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut file = crate::windows_private::open_private_file(&path)?;
                let mut token = String::new();
                file.read_to_string(&mut token)?;
                token
            }
            Err(error) => return Err(error.into()),
        };
        Ok((Self::new(token)?, path))
    }

    #[cfg(unix)]
    pub fn open(database_path: &Path) -> Result<(Self, std::path::PathBuf)> {
        let path = database_path.with_extension("access-token");
        let token = match fs::symlink_metadata(&path) {
            Ok(_) => {
                let mut options = fs::OpenOptions::new();
                options.read(true);
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
                let mut file = options.open(&path).context("Open dashboard access token")?;
                let metadata = file.metadata()?;
                if !metadata.file_type().is_file() {
                    bail!("Dashboard access token must be a regular file");
                }
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.permissions().mode() & 0o077 != 0 {
                    bail!("Dashboard access token must be readable only by its owner");
                }
                // Running Observer as a different account must not accept
                // a token file planted in a shared workspace directory.
                if metadata.uid() != unsafe { libc::geteuid() } {
                    bail!("Dashboard access token must belong to the current user");
                }
                let mut token = String::new();
                file.read_to_string(&mut token)
                    .context("Read dashboard access token")?;
                token
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let token = format!(
                    "{PREFIX}{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
                let mut file = options
                    .open(&path)
                    .context("Create dashboard access token")?;
                file.write_all(token.as_bytes())?;
                file.sync_all()?;
                token
            }
            Err(error) => return Err(error).context("Inspect dashboard access token"),
        };
        Ok((Self::new(token)?, path))
    }

    pub fn allows_basic(&self, value: Option<&str>) -> bool {
        value.is_some_and(|value| same_bytes(value.as_bytes(), self.basic.as_bytes()))
    }

    pub fn allows_token(&self, value: Option<&str>) -> bool {
        value.is_some_and(|value| same_bytes(value.as_bytes(), self.token.as_bytes()))
    }

    #[cfg(test)]
    pub fn test() -> Self {
        Self::new(format!("{PREFIX}{}", "a".repeat(64))).unwrap()
    }

    #[cfg(test)]
    pub fn token(&self) -> &str {
        &self.token
    }

    #[cfg(test)]
    pub fn basic_header(&self) -> &str {
        &self.basic
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn token_file_is_private_and_reused() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("observer.sqlite");
        let (first, path) = Access::open(&database).unwrap();
        let (second, _) = Access::open(&database).unwrap();
        assert!(second.allows_token(Some(first.token())));
        assert!(!second.allows_token(Some("wrong")));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Access::open(&database).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_access_token_is_rejected() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("observer.sqlite");
        let target = directory.path().join("other-token");
        fs::write(&target, format!("{PREFIX}{}", "a".repeat(64))).unwrap();
        symlink(&target, database.with_extension("access-token")).unwrap();
        assert!(Access::open(&database).is_err());
    }
}
