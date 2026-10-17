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
const TOKEN_LENGTH: usize = PREFIX.len() + 64;

#[cfg(any(unix, windows))]
fn read_token(file: impl Read) -> Result<String> {
    let mut token = String::new();
    // Read one extra byte so a valid prefix followed by trailing content is
    // rejected without allocating for an arbitrarily large malformed file.
    file.take((TOKEN_LENGTH + 1) as u64)
        .read_to_string(&mut token)?;
    Ok(token)
}

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
            || token.len() != TOKEN_LENGTH
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
                read_token(crate::windows_private::open_private_file(&path)?)?
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
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                let file = options.open(&path).context("Open dashboard access token")?;
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
                read_token(file).context("Read dashboard access token")?
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

    #[test]
    fn token_reads_are_bounded_and_reject_trailing_content() {
        use std::io::Cursor;
        let valid = format!("{PREFIX}{}", "a".repeat(64));
        assert!(Access::new(read_token(Cursor::new(valid.as_bytes())).unwrap()).is_ok());
        let mut oversized = Cursor::new(format!("{valid}{}", "x".repeat(4096)));
        let retained = read_token(&mut oversized).unwrap();
        assert_eq!(oversized.position(), (TOKEN_LENGTH + 1) as u64);
        assert!(Access::new(retained).is_err());
        for malformed in [
            "".to_owned(),
            format!("{valid}\n"),
            "a".repeat(TOKEN_LENGTH),
        ] {
            assert!(Access::new(read_token(Cursor::new(malformed)).unwrap()).is_err());
        }
    }

    #[test]
    fn named_pipe_access_token_is_rejected_without_blocking() {
        const CHILD_PATH: &str = "JEV_OBSERVER_FIFO_TEST_PATH";
        if let Some(database) = std::env::var_os(CHILD_PATH) {
            assert!(Access::open(Path::new(&database)).is_err());
            return;
        }
        use std::{
            ffi::CString,
            os::unix::ffi::OsStrExt,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("observer.sqlite");
        let path = CString::new(
            database
                .with_extension("access-token")
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        // Isolate the open in a child so a blocking regression cannot hang the
        // entire suite or leave a test thread waiting on the pipe forever.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "access::tests::named_pipe_access_token_is_rejected_without_blocking",
            ])
            .env(CHILD_PATH, &database)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if started.elapsed() > Duration::from_secs(2) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Opening a named-pipe access token blocked startup");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
