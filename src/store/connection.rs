use super::Store;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
#[cfg(unix)]
use std::path::PathBuf;
use std::{io::Read, path::Path, sync::Arc, time::Duration};

const DASHBOARD_INDEX: &str = "CREATE INDEX request_dashboard ON requests(timestamp,source,model,status,event_kind,answer_count,input_tokens,output_tokens,cost_usd,sample,capture_complete,duration_ms,json_extract(data,'$.timestamp'),id,json_extract(data,'$.transport_error'))";

fn check_schema(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='schema_version')",
        [],
        |row| row.get(0),
    )?;
    if exists {
        let versions = conn
            .prepare("SELECT version FROM schema_version LIMIT 2")?
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !versions.is_empty() && versions != [1] {
            bail!("Unsupported database schema {versions:?}; use a compatible Observer version");
        }
    }
    Ok(())
}

pub(super) fn plaintext_database(path: &Path) -> Result<bool> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Follow supported database symlinks, but never wait for a FIFO writer.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Inspect existing history"),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        bail!("History database must be a regular file");
    }
    if metadata.len() == 0 {
        return Ok(false);
    }
    let mut header = [0_u8; 16];
    file.read_exact(&mut header)
        .context("Read history header")?;
    Ok(&header == b"SQLite format 3\0")
}

#[cfg(unix)]
fn check_database_peers(path: &Path) -> Result<()> {
    let database = std::fs::canonicalize(path)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut peer = database.as_os_str().to_owned();
        peer.push(suffix);
        let peer = PathBuf::from(peer);
        match std::fs::symlink_metadata(&peer) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                bail!(
                    "Database journal must be a regular file: {}",
                    peer.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Inspect database journal before reading"),
        }
    }
    Ok(())
}

fn migrate_plaintext(path: &Path, key: &str) -> Result<()> {
    // Opening a future database must be a read-only operation, including the
    // live plaintext-upgrade path. Do this before permissions, journals or files change.
    #[cfg(unix)]
    check_database_peers(path)?;
    let preflight = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    check_schema(&preflight)?;
    drop(preflight);
    eprintln!("Encrypting existing Observer history: {}", path.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".jev-observer-migrate-")
        .tempfile_in(parent)
        .context("Create encrypted migration file")?;
    let output = temporary.path().to_owned();
    let source = Connection::open(path).context("Open plaintext history for migration")?;
    check_schema(&source)?;
    source.execute_batch("PRAGMA temp_store=MEMORY;")?;
    // Checkpoint every saved record before replacing the database file. A
    // second running Observer process must be stopped before this migration.
    let busy: i64 = source.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        bail!("Close other Observer processes before migrating history");
    }
    let journal_mode: String =
        source.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
    if journal_mode != "delete" {
        bail!("Could not close plaintext SQLite journal before migration");
    }
    source.execute(
        "ATTACH DATABASE ?1 AS encrypted KEY ?2",
        params![
            output
                .to_str()
                .context("Non-UTF-8 database migration path")?,
            format!("x'{key}'")
        ],
    )?;
    source.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
    source.execute_batch("DETACH DATABASE encrypted;")?;
    drop(source);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut peer = path.as_os_str().to_owned();
        peer.push(suffix);
        if Path::new(&peer).exists() {
            bail!(
                "Close other Observer processes and remove remaining SQLite sidecars before migrating history"
            );
        }
    }
    let check = Connection::open(&output)?;
    check.pragma_update(None, "key", format!("x'{key}'"))?;
    let integrity: String = check.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        bail!("Encrypted migration failed integrity check");
    }
    drop(check);
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .context("Replace plaintext history with encrypted history")?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    eprintln!("Observer history encryption completed");
    Ok(())
}

impl Store {
    pub fn key_from_environment() -> Result<String> {
        let key = std::env::var("JEV_OBSERVER_DB_KEY")
            .context("Set JEV_OBSERVER_DB_KEY to 64 random hexadecimal characters before starting live collection")?;
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("JEV_OBSERVER_DB_KEY must contain exactly 64 hexadecimal characters");
        }
        Ok(key)
    }

    pub fn open_encrypted(
        path: &Path,
        key: String,
        retention_days: u32,
        max_records: usize,
    ) -> Result<Self> {
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Database encryption key must contain exactly 64 hexadecimal characters");
        }
        #[cfg(windows)]
        {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            crate::windows_private::prepare_database_directory(path)?;
        }
        let migration_path = if path.exists() {
            path.canonicalize()
                .context("Resolve history path before migration")?
        } else {
            path.to_owned()
        };
        if plaintext_database(&migration_path)? {
            migrate_plaintext(&migration_path, &key)?;
        }
        Self::open_inner(path, Some(key), retention_days, max_records)
    }

    #[cfg(test)]
    pub fn open(path: &Path, retention_days: u32, max_records: usize) -> Result<Self> {
        Self::open_inner(path, None, retention_days, max_records)
    }

    pub fn open_demo(path: &Path, retention_days: u32, max_records: usize) -> Result<Self> {
        Self::open_inner(path, None, retention_days, max_records)
    }

    fn open_inner(
        path: &Path,
        key: Option<String>,
        retention_days: u32,
        max_records: usize,
    ) -> Result<Self> {
        if max_records == 0 || retention_days == 0 || i64::try_from(max_records).is_err() {
            bail!("Retention days and record limit must be positive");
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(windows)]
        crate::windows_private::prepare_database_directory(path)?;
        let store = Self {
            path: Arc::new(path.to_owned()),
            key: key.map(Arc::new),
            retention_days,
            max_records,
        };
        let existing = match std::fs::metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    bail!("History database must be a regular file");
                }
                metadata.len() > 0
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).context("Inspect existing history before opening"),
        };
        if existing {
            // Closing even an untouched READ_WRITE connection can checkpoint
            // a crash-left WAL. Reject future schemas before any writer opens.
            // READ_ONLY honors committed WAL state; immutable mode does not.
            #[cfg(unix)]
            check_database_peers(path)?;
            let preflight = store.reader()?;
            check_schema(&preflight)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
            // Set owner-only access before SQLite creates its WAL/SHM peers.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            // Recheck the opened descriptor before chmod if the path changed
            // after the preflight. Regular database symlinks remain supported.
            if !file.metadata()?.file_type().is_file() {
                bail!("History database must be a regular file");
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            // Existing journals may have arrived with permissive backup modes.
            // SQLite does not tighten their modes when the main file changes.
            // Its Unix VFS resolves database symlinks before naming these peers.
            let database_path = std::fs::canonicalize(path)?;
            for suffix in ["-wal", "-shm", "-journal"] {
                let mut peer = database_path.as_os_str().to_owned();
                peer.push(suffix);
                let peer = PathBuf::from(peer);
                let metadata = match std::fs::symlink_metadata(&peer) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("Inspect database journal permissions");
                    }
                };
                if !metadata.file_type().is_file() {
                    bail!(
                        "Database journal must be a regular file: {}",
                        peer.display()
                    );
                }
                let file = match std::fs::File::open(&peer) {
                    Ok(file) => file,
                    // Another connection can remove a journal while closing.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("Open database journal for permission repair");
                    }
                };
                let opened = file.metadata()?;
                if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
                    bail!(
                        "Database journal changed while securing permissions: {}",
                        peer.display()
                    );
                }
                // Apply permissions to the checked descriptor, not a path that
                // could have been replaced with an unrelated symlink target.
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
        }
        let mut conn = store.writer_connection()?;
        check_schema(&conn)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
            INSERT INTO schema_version SELECT 1 WHERE NOT EXISTS(SELECT 1 FROM schema_version);
            CREATE TABLE IF NOT EXISTS requests(
              seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT NOT NULL UNIQUE,timestamp INTEGER NOT NULL,
              source TEXT NOT NULL,model TEXT NOT NULL,status INTEGER,duration_ms REAL,
              input_tokens INTEGER,output_tokens INTEGER,cost_usd REAL,cost_basis TEXT NOT NULL,
              answer_count INTEGER NOT NULL,capture_complete INTEGER NOT NULL,sample INTEGER NOT NULL,
              source_event_id TEXT,import_format TEXT,event_kind TEXT NOT NULL DEFAULT 'request',data TEXT NOT NULL);
            CREATE UNIQUE INDEX IF NOT EXISTS request_import_identity ON requests(source,import_format,source_event_id) WHERE source_event_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS request_time ON requests(timestamp);
            CREATE INDEX IF NOT EXISTS request_source_time ON requests(source,timestamp);
            CREATE INDEX IF NOT EXISTS request_model_time ON requests(model,timestamp);
            CREATE INDEX IF NOT EXISTS request_join_scope ON requests(id,timestamp,source,model,status);
            CREATE INDEX IF NOT EXISTS request_dashboard ON requests(timestamp,source,model,status,event_kind,answer_count,input_tokens,output_tokens,cost_usd,sample,capture_complete,duration_ms,json_extract(data,'$.timestamp'),id,json_extract(data,'$.transport_error'));
            CREATE TABLE IF NOT EXISTS groups(
              id TEXT PRIMARY KEY,key TEXT NOT NULL,kind TEXT NOT NULL,source TEXT NOT NULL,
              definition_id TEXT NOT NULL,presentation_id TEXT NOT NULL,definition TEXT NOT NULL,
              task_version TEXT,family_id TEXT,family_name TEXT,adapter TEXT);
            CREATE INDEX IF NOT EXISTS group_versions ON groups(source,key);
            CREATE INDEX IF NOT EXISTS group_family ON groups(family_id);
            CREATE TABLE IF NOT EXISTS answers(
              request_id TEXT NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
              key TEXT NOT NULL,group_id TEXT NOT NULL REFERENCES groups(id),kind TEXT NOT NULL,
              valid INTEGER NOT NULL,value_num REAL,value_text TEXT,confidence REAL,data TEXT NOT NULL,
              PRIMARY KEY(request_id,key));
            CREATE INDEX IF NOT EXISTS answer_statistics ON answers(group_id,request_id,kind,valid,value_num,value_text,confidence);
            CREATE INDEX IF NOT EXISTS answer_overview ON answers((CASE WHEN length(group_id)=73 AND length(CAST(group_id AS BLOB))=73 AND substr(group_id,1,9)='group_v1_' AND substr(group_id,10) NOT GLOB '*[^0-9a-f]*' THEN unhex(substr(group_id,10)) ELSE group_id END),request_id,kind,valid,value_num,value_text,confidence);
            CREATE INDEX IF NOT EXISTS answer_key ON answers(key,request_id);
            DROP INDEX IF EXISTS answer_group;
            CREATE TABLE IF NOT EXISTS labels(
              request_id TEXT NOT NULL REFERENCES requests(id) ON DELETE CASCADE,key TEXT NOT NULL,
              label TEXT NOT NULL,timestamp INTEGER NOT NULL,source TEXT NOT NULL,annotation TEXT,
              PRIMARY KEY(request_id,key));
            CREATE TABLE IF NOT EXISTS maintenance(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS credential_approval(
              id INTEGER PRIMARY KEY CHECK(id=1), token_hash TEXT NOT NULL);")?;
        // Additive: older binaries use explicit column lists. Their local-review
        // INSERT OR REPLACE also clears annotation, just like current writes.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let has_annotation: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('labels') WHERE name='annotation')",
            [],
            |row| row.get(0),
        )?;
        if !has_annotation {
            tx.execute_batch("ALTER TABLE labels ADD COLUMN annotation TEXT")?;
        }
        tx.commit()?;
        // Upgrade the covering index once, transactionally, for transfer outcomes.
        let index_sql: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='request_dashboard'",
            [],
            |row| row.get(0),
        )?;
        if !index_sql.contains("transport_error") {
            let tx = conn.transaction()?;
            tx.execute_batch("DROP INDEX request_dashboard;")?;
            tx.execute_batch(DASHBOARD_INDEX)?;
            tx.commit()?;
        }
        // Enforce the configured policy before exposing existing history, even
        // when no new provider requests arrive after a restart.
        store.maintenance(&mut conn, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(store)
    }

    pub fn writer_connection(&self) -> Result<Connection> {
        let conn = Connection::open(self.path.as_ref()).context("Open local database")?;
        self.apply_key(&conn)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        // Negative cache_size is a KiB budget, independent of database page size.
        // Keep hot index pages resident while the writer appends capture batches.
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-32768;",
        )?;
        Ok(conn)
    }

    pub(super) fn reader(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            self.path.as_ref(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        self.apply_key(&conn)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        // A read snapshot can also sort retained definitions and metadata.
        // Keep all such temporary payloads in RAM, including demo readers.
        conn.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA cache_size=-8192;")?;
        Ok(conn)
    }

    fn apply_key(&self, conn: &Connection) -> Result<()> {
        if let Some(key) = &self.key {
            conn.pragma_update(None, "key", format!("x'{key}'"))?;
            // SQLCipher accepts PRAGMA key before checking it. Force a read
            // before any schema or journal operation can change the file.
            conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
                row.get::<_, i64>(0)
            })
            .context("Cannot unlock encrypted history; check JEV_OBSERVER_DB_KEY")?;
            conn.execute_batch("PRAGMA temp_store=MEMORY;")?;
        }
        Ok(())
    }
}
