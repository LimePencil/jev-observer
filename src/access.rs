#[cfg(any(unix, windows))]
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
fn token_path(database_path: &Path) -> std::path::PathBuf {
    // Preserve the complete OS filename: replacing an extension shares authority
    // between distinct databases such as history.sqlite and history.db.
    let mut path = database_path.as_os_str().to_os_string();
    path.push(".token");
    path.into()
}

#[cfg(any(unix, windows))]
fn note_legacy_token(database_path: &Path) {
    // Inspect only the old directory entry, never its token contents. Reusing an
    // old token would preserve authority shared by same-stem databases.
    if fs::symlink_metadata(database_path.with_extension("access-token")).is_ok() {
        eprintln!(
            "Created a fresh workspace access token; sign in again and update clients that use the workspace access token. The legacy access-token file was left unchanged for rollback."
        );
    }
}

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
        let path = token_path(database_path);
        let token = match crate::windows_private::create_private_file(&path) {
            Ok(mut file) => {
                let token = format!(
                    "{PREFIX}{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                file.write_all(token.as_bytes())?;
                file.sync_all()?;
                note_legacy_token(database_path);
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
        let path = token_path(database_path);
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
                note_legacy_token(database_path);
                token
            }
            Err(error) => return Err(error).context("Inspect dashboard access token"),
        };
        Ok((Self::new(token)?, path))
    }

    pub fn allows_basic(&self, value: Option<&str>) -> bool {
        value
            .and_then(|value| value.split_once(' '))
            .is_some_and(|(scheme, credential)| {
                // HTTP auth schemes are case-insensitive and use one or more
                // ASCII spaces. Only that syntax is normalized: the encoded
                // credential remains an exact, constant-time comparison.
                scheme.eq_ignore_ascii_case("Basic")
                    && same_bytes(
                        credential.trim_start_matches(' ').as_bytes(),
                        &self.basic.as_bytes()["Basic ".len()..],
                    )
            })
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

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    #[test]
    fn complete_database_filenames_have_distinct_stable_tokens() {
        let directory = tempfile::tempdir().unwrap();
        let names = [
            "history.sqlite",
            "history.db",
            "history",
            ".history",
            "history.part.sqlite",
        ]
        .map(std::ffi::OsString::from)
        .to_vec();
        // The macOS release filesystem rejects invalid UTF-8 filenames.
        #[cfg(target_os = "linux")]
        let names = {
            use std::os::unix::ffi::OsStringExt;
            let mut names = names;
            names.push(std::ffi::OsString::from_vec(b"history.\xff".to_vec()));
            names
        };
        let mut tokens: Vec<Access> = Vec::new();
        for name in names {
            let database = directory.path().join(&name);
            let (access, path) = Access::open(&database).unwrap();
            let mut expected_name = name;
            expected_name.push(".token");
            assert_eq!(path, directory.path().join(expected_name));
            for previous in &tokens {
                assert!(!access.allows_token(Some(previous.token())));
                assert!(!access.allows_basic(Some(previous.basic_header())));
            }
            let (reopened, reopened_path) = Access::open(&database).unwrap();
            assert_eq!(path, reopened_path);
            assert!(reopened.allows_token(Some(access.token())));
            tokens.push(access);
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_paths_preserve_native_bytes_without_filesystem_access() {
        use std::os::unix::ffi::OsStrExt;

        // This byte-preservation contract applies even where the filesystem
        // rejects invalid UTF-8 names, as on the macOS release runner.
        let cases: [(&[u8], &[u8]); 3] = [
            (b"workspace/history.\xff", b"workspace/history.\xff.token"),
            (b"workspace/history.\xfe", b"workspace/history.\xfe.token"),
            (
                b"workspace/history.\xef\xbf\xbd",
                b"workspace/history.\xef\xbf\xbd.token",
            ),
        ];
        let paths: Vec<_> = cases
            .iter()
            .map(|(input, expected)| {
                let path = token_path(Path::new(std::ffi::OsStr::from_bytes(input)));
                assert_eq!(path.as_os_str().as_bytes(), *expected);
                path
            })
            .collect();
        assert_eq!(paths[0].to_string_lossy(), paths[1].to_string_lossy());
        assert_eq!(paths[0].to_string_lossy(), paths[2].to_string_lossy());
        assert_ne!(paths[0], paths[1]);
        assert_ne!(paths[0], paths[2]);
        assert_ne!(paths[1], paths[2]);
    }

    #[cfg(unix)]
    #[test]
    fn legacy_token_is_ignored_and_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("history.sqlite");
        let legacy_path = database.with_extension("access-token");
        let legacy = Access::test();
        fs::write(&legacy_path, legacy.token()).unwrap();
        fs::set_permissions(&legacy_path, fs::Permissions::from_mode(0o600)).unwrap();
        let (current, path) = Access::open(&database).unwrap();
        assert_ne!(path, legacy_path);
        assert!(!current.allows_token(Some(legacy.token())));
        assert!(!current.allows_basic(Some(legacy.basic_header())));
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy.token().as_bytes());
        assert_eq!(
            fs::metadata(&legacy_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // An existing new token remains authoritative even if the unused old
        // file later becomes malformed or has permissive permissions.
        fs::write(&legacy_path, "unused legacy contents").unwrap();
        fs::set_permissions(&legacy_path, fs::Permissions::from_mode(0o644)).unwrap();
        let (reopened, _) = Access::open(&database).unwrap();
        assert!(reopened.allows_token(Some(current.token())));
        assert_eq!(
            fs::read_to_string(&legacy_path).unwrap(),
            "unused legacy contents"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_and_symlinked_legacy_files_are_not_opened() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("history.sqlite");
        let legacy_path = database.with_extension("access-token");
        fs::write(&legacy_path, "not a valid token").unwrap();
        assert!(Access::open(&database).is_ok());
        fs::remove_file(token_path(&database)).unwrap();
        fs::remove_file(&legacy_path).unwrap();
        let absent = directory.path().join("absent-target");
        symlink(&absent, &legacy_path).unwrap();
        assert!(Access::open(&database).is_ok());
        assert_eq!(fs::read_link(&legacy_path).unwrap(), absent);
        assert!(!absent.exists());
    }

    #[test]
    fn legacy_rotation_notice_is_emitted_once_without_token_contents() {
        const CHILD_PATH: &str = "JEV_OBSERVER_TOKEN_NOTICE_TEST_PATH";
        if let Some(database) = std::env::var_os(CHILD_PATH) {
            Access::open(Path::new(&database)).unwrap();
            Access::open(Path::new(&database)).unwrap();
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("history.sqlite");
        let marker = "legacy-token-content-must-not-be-logged";
        fs::write(database.with_extension("access-token"), marker).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "access::tests::legacy_rotation_notice_is_emitted_once_without_token_contents",
                "--nocapture",
            ])
            .env(CHILD_PATH, &database)
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr
                .matches("Created a fresh workspace access token;")
                .count(),
            1
        );
        assert!(!stderr.contains(marker));
        assert!(!stderr.contains(&fs::read_to_string(token_path(&database)).unwrap()));
    }

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
        symlink(&target, token_path(&database)).unwrap();
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

    #[cfg(unix)]
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
        let path = CString::new(token_path(&database).as_os_str().as_bytes()).unwrap();
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

#[cfg(test)]
mod basic_tests {
    use super::*;

    #[test]
    fn basic_scheme_is_case_insensitive() {
        let access = Access::test();
        let credential = access.basic_header().strip_prefix("Basic ").unwrap();
        for scheme in ["Basic", "basic", "BASIC", "bAsIc"] {
            assert!(access.allows_basic(Some(&format!("{scheme} {credential}"))));
        }
    }

    #[test]
    fn basic_separator_accepts_multiple_ascii_spaces() {
        let access = Access::test();
        let credential = access.basic_header().strip_prefix("Basic ").unwrap();
        for separator in [" ", "  ", "    "] {
            assert!(access.allows_basic(Some(&format!("Basic{separator}{credential}"))));
        }
    }

    #[test]
    fn basic_credentials_remain_exact_and_case_sensitive() {
        let access = Access::test();
        let credential = access.basic_header().strip_prefix("Basic ").unwrap();
        let mut changed_case = credential.as_bytes().to_vec();
        let letter = changed_case
            .iter_mut()
            .find(|byte| byte.is_ascii_alphabetic())
            .unwrap();
        *letter = if letter.is_ascii_lowercase() {
            letter.to_ascii_uppercase()
        } else {
            letter.to_ascii_lowercase()
        };
        let changed_case = String::from_utf8(changed_case).unwrap();
        let wrong_user = STANDARD.encode(format!("Observer:{}", access.token()));
        let wrong_token = STANDARD.encode(format!("observer:{PREFIX}{}", "b".repeat(64)));
        for malformed in [
            format!("Basic {changed_case}"),
            format!("Basic {wrong_user}"),
            format!("Basic {wrong_token}"),
            format!("Basic {credential} trailing"),
            format!("Basic {credential} "),
            format!("Basic {credential}\t"),
            format!("Basic {credential}="),
        ] {
            assert!(
                !access.allows_basic(Some(&malformed)),
                "altered credential accepted"
            );
        }
    }

    #[test]
    fn basic_separator_rejects_tabs_and_missing_components() {
        let access = Access::test();
        let credential = access.basic_header().strip_prefix("Basic ").unwrap();
        assert!(!access.allows_basic(None));
        for malformed in [
            String::new(),
            "Basic".into(),
            "Basic ".into(),
            "Basic    ".into(),
            format!("Basic\t{credential}"),
            format!("Basic \t{credential}"),
            format!("Basic\t {credential}"),
            format!("Basic\u{00a0}{credential}"),
            format!(" Basic {credential}"),
            format!("Bearer {credential}"),
            format!("BasicX {credential}"),
        ] {
            assert!(
                !access.allows_basic(Some(&malformed)),
                "invalid Basic syntax accepted"
            );
        }
    }
}
