use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub struct Store {
    path: Arc<PathBuf>,
    key: Option<Arc<String>>,
    retention_days: u32,
    max_records: usize,
}

#[derive(Clone, Default, Deserialize)]
pub struct Filter {
    /// Internal snapshot time; callers cannot set it through HTTP query strings.
    #[serde(skip)]
    pub as_of: Option<i64>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub window: Option<String>,
    pub group: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub request_cursor: Option<String>,
    pub group_cursor: Option<String>,
    pub group_search: Option<String>,
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn optional_text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str()
}
fn parse(s: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[derive(Default)]
struct Mean {
    sum: f64,
    correction: f64,
    count: u64,
}

impl Mean {
    fn add(&mut self, value: f64) {
        // Compensated summation matches SQLite AVG's treatment of REAL values.
        let next = self.sum + value;
        self.correction += if self.sum.abs() > value.abs() {
            (self.sum - next) + value
        } else {
            (value - next) + self.sum
        };
        self.sum = next;
        self.count += 1;
    }

    fn total(&self) -> Option<f64> {
        (self.count > 0).then_some(self.sum + self.correction)
    }

    fn value(&self) -> Option<f64> {
        self.total().map(|sum| sum / self.count as f64)
    }
}

struct DashboardRequests {
    summary: Value,
    timeline: Vec<Value>,
    parents: HashMap<String, i64>,
    timeline_meta: Value,
}

#[derive(Default)]
struct TimelineBucket {
    requests: i64,
    errors: i64,
    latency: Mean,
    cost: Mean,
}

const DASHBOARD_SELECTION: &str = "r.seq IN (SELECT seq FROM temp.dashboard_request_ids)";
const FAILED: &str = "(r.status>=400 OR json_extract(r.data,'$.transport_error') IS NOT NULL)";
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

struct TimelineSpec {
    start: i64,
    end: i64,
    width: i64,
    count: usize,
}

impl TimelineSpec {
    fn bucket(&self, timestamp: i64) -> i64 {
        let offset = (i128::from(timestamp) - i128::from(self.start)) / i128::from(self.width);
        (i128::from(self.start) + offset * i128::from(self.width)) as i64
    }

    fn metadata(&self) -> Value {
        json!({"start":self.start,"end":self.end.saturating_add(1),"bucket_width":self.width,"truncated":false})
    }

    fn finish(&self, mut buckets: BTreeMap<i64, TimelineBucket>) -> Vec<Value> {
        (0..self.count)
            .map(|index| {
                let timestamp =
                    (i128::from(self.start) + index as i128 * i128::from(self.width)) as i64;
                let bucket = buckets.remove(&timestamp).unwrap_or_default();
                json!({"timestamp":timestamp,"requests":bucket.requests,"errors":bucket.errors,
                "mean_latency_ms":bucket.latency.value(),"cost_usd":bucket.cost.total()})
            })
            .collect()
    }
}

fn plaintext_database(path: &Path) -> Result<bool> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Inspect existing history"),
    };
    if file.metadata()?.len() == 0 {
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

fn add_integer_sum(total: &mut Option<i64>, value: Option<i64>) -> Result<()> {
    if let Some(value) = value {
        *total = Some(
            total
                .unwrap_or(0)
                .checked_add(value)
                .context("integer overflow")?,
        );
    }
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
            Ok(metadata) => metadata.len() > 0,
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
                .open(path)?;
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
            DROP INDEX IF EXISTS answer_group;
            CREATE TABLE IF NOT EXISTS labels(
              request_id TEXT NOT NULL REFERENCES requests(id) ON DELETE CASCADE,key TEXT NOT NULL,
              label TEXT NOT NULL,timestamp INTEGER NOT NULL,source TEXT NOT NULL,
              PRIMARY KEY(request_id,key));
            CREATE TABLE IF NOT EXISTS maintenance(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS credential_approval(
              id INTEGER PRIMARY KEY CHECK(id=1), token_hash TEXT NOT NULL);")?;
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

    fn reader(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            self.path.as_ref(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        self.apply_key(&conn)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.execute_batch("PRAGMA cache_size=-8192;")?;
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

    /// Only a digest of a random local token is stored here. It prevents an
    /// obsolete system credential from becoming active after a locked store
    /// prevented its immediate deletion.
    pub fn credential_approval(&self) -> Result<Option<String>> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT token_hash FROM credential_approval WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_credential_approval(&self, token_hash: Option<&str>) -> Result<()> {
        let connection = self.writer_connection()?;
        if let Some(token_hash) = token_hash {
            connection.execute(
                "INSERT INTO credential_approval(id,token_hash) VALUES(1,?) \
                 ON CONFLICT(id) DO UPDATE SET token_hash=excluded.token_hash",
                [token_hash],
            )?;
        } else {
            connection.execute("DELETE FROM credential_approval WHERE id=1", [])?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self
            .reader()?
            .query_row("SELECT NOT EXISTS(SELECT 1 FROM requests)", [], |row| {
                row.get(0)
            })?)
    }

    pub fn write_batch(&self, conn: &mut Connection, records: &[Value]) -> Result<usize> {
        let tx = conn.transaction()?;
        let mut inserted = 0;
        for record in records {
            let id = text(record, "id");
            if id.is_empty() {
                bail!("Record ID is required");
            }
            let mut parent = record.clone();
            parent
                .as_object_mut()
                .context("Record must be an object")?
                .remove("answers");
            let answers = record["answers"]
                .as_array()
                .context("Record answers must be an array")?;
            let count = tx.prepare_cached("INSERT OR IGNORE INTO requests(id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample,source_event_id,import_format,event_kind,data) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")?.execute(
                params![id, record["timestamp"].as_i64().or(record["imported_at"].as_i64()).context("timestamp or imported_at is required")?, text(record,"source"),text(record,"model"),record["status"].as_i64(),record["duration_ms"].as_f64(),record["input_tokens"].as_i64(),record["output_tokens"].as_i64(),record["cost_usd"].as_f64(),text(record,"cost_basis"),answers.len() as i64,record["capture_complete"].as_bool().unwrap_or(false),record["sample"].as_bool().unwrap_or(false),optional_text(record,"source_event_id"),optional_text(record,"import_format"),record["event_kind"].as_str().unwrap_or("request"),serde_json::to_string(&parent)?])?;
            if count == 0 {
                continue;
            }
            inserted += 1;
            for answer in answers {
                tx.prepare_cached("INSERT OR IGNORE INTO groups(id,key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter) VALUES(?,?,?,?,?,?,?,?,?,?,?)")?.execute( params![text(answer,"group_id"),text(answer,"key"),text(answer,"kind"),text(record,"source"),text(answer,"definition_id"),text(answer,"presentation_id"),serde_json::to_string(&answer["definition"])?,optional_text(answer,"task_version"),optional_text(answer,"family_id"),optional_text(answer,"family_name"),optional_text(answer,"adapter")])?;
                let mut child = answer.clone();
                child
                    .as_object_mut()
                    .context("Answer must be an object")?
                    .remove("definition");
                tx.prepare_cached("INSERT INTO answers(request_id,key,group_id,kind,valid,value_num,value_text,confidence,data) VALUES(?,?,?,?,?,?,?,?,?)")?.execute(params![id,text(answer,"key"),text(answer,"group_id"),text(answer,"kind"),answer["valid"].as_bool().unwrap_or(false),answer["value"].as_f64(),answer["value"].as_str(),answer["confidence"].as_f64(),serde_json::to_string(&child)?])?;
            }
            if let Some(labels) = record["labels"].as_array() {
                for label in labels {
                    let key = label["key"].as_str().or(label["question_key"].as_str());
                    let value = label["label"].as_str();
                    if let (Some(key), Some(value)) = (key, value)
                        && ["correct", "incorrect", "unknown"].contains(&value)
                    {
                        tx.prepare_cached("INSERT OR REPLACE INTO labels(request_id,key,label,timestamp,source) VALUES(?,?,?,?,?)")?.execute(params![id,key,value,label["timestamp"].as_i64().unwrap_or_else(|| Utc::now().timestamp_millis()),label["source"].as_str().unwrap_or("import")])?;
                    }
                }
            }
        }
        self.prune_in_transaction(&tx, false)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Run age/count retention without requiring a new captured event.
    pub fn maintenance(&self, conn: &mut Connection, force: bool) -> Result<bool> {
        let tx = conn.transaction()?;
        let ran = self.prune_in_transaction(&tx, force)?;
        tx.commit()?;
        Ok(ran)
    }

    fn prune_in_transaction(&self, tx: &Connection, force: bool) -> Result<bool> {
        let now = Utc::now().timestamp_millis();
        let last: i64 = tx
            .query_row(
                "SELECT value FROM maintenance WHERE key='pruned_at'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if force || now < last || now - last >= 30_000 {
            tx.execute(
                "DELETE FROM requests WHERE timestamp < ?",
                [now - i64::from(self.retention_days) * 86_400_000],
            )?;
            tx.execute("DELETE FROM requests WHERE seq <= COALESCE((SELECT seq FROM requests ORDER BY seq DESC LIMIT 1 OFFSET ?),-1)", [self.max_records as i64])?;
            tx.execute("DELETE FROM groups WHERE NOT EXISTS(SELECT 1 FROM answers a WHERE a.group_id=groups.id)", [])?;
            tx.execute(
                "INSERT OR REPLACE INTO maintenance(key,value) VALUES('pruned_at',?)",
                [now],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    fn predicate(filter: &Filter) -> Result<(String, Vec<SqlValue>)> {
        Self::request_predicate(filter, false)
    }

    fn request_predicate(filter: &Filter, batch_search: bool) -> Result<(String, Vec<SqlValue>)> {
        let mut clauses = vec!["1=1".to_owned()];
        let mut args = Vec::new();
        let period = match filter.window.as_deref().unwrap_or("24h") {
            "1h" => Some(3_600_000),
            "24h" => Some(86_400_000),
            "7d" => Some(604_800_000),
            "all" => None,
            _ => bail!("Unknown time window"),
        };
        if let Some(period) = period.filter(|_| filter.from.is_none() && filter.to.is_none()) {
            clauses.push("r.timestamp>=? AND r.timestamp<=?".into());
            let as_of = filter
                .as_of
                .unwrap_or_else(|| Utc::now().timestamp_millis());
            args.push((as_of - period).into());
            args.push(as_of.into());
        }
        if let Some(from) = filter.from {
            clauses.push("r.timestamp>=?".into());
            args.push(from.into());
        }
        if let Some(to) = filter.to {
            clauses.push("r.timestamp<=?".into());
            args.push(to.into());
        }
        if filter
            .from
            .zip(filter.to)
            .is_some_and(|(from, to)| from > to)
        {
            bail!("Date range start must not follow its end");
        }
        for (name, value) in [("source", &filter.source), ("model", &filter.model)] {
            if let Some(value) = value.as_ref().filter(|s| !s.is_empty()) {
                clauses.push(format!("r.{name}=?"));
                args.push(value.clone().into());
            }
        }
        if filter.status.as_deref() == Some("error") {
            clauses.push(FAILED.into());
        }
        if let Some(group) = filter.group.as_ref().filter(|group| !group.is_empty()) {
            clauses.push("EXISTS(SELECT 1 FROM answers ga JOIN groups gg ON gg.id=ga.group_id WHERE ga.request_id=r.id AND (gg.id=? OR gg.family_id=?))".into());
            args.push(group.clone().into());
            args.push(group.clone().into());
        }
        if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
            let answer_match = if batch_search {
                "r.id IN (SELECT sa.request_id FROM answers sa WHERE instr(lower(sa.key),lower(?))>0)"
            } else {
                "EXISTS(SELECT 1 FROM answers sa WHERE sa.request_id=r.id AND instr(lower(sa.key),lower(?))>0)"
            };
            clauses.push(format!("(instr(lower(r.source),lower(?))>0 OR instr(lower(r.id),lower(?))>0 OR {answer_match})"));
            for _ in 0..3 {
                args.push(search.clone().into());
            }
        }
        Ok((clauses.join(" AND "), args))
    }

    fn request_page(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        cursor: Option<&str>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let mut scope = predicate.to_owned();
        let mut values = args.to_vec();
        if let Some(cursor) = cursor {
            let (timestamp, seq) = cursor.split_once(':').context("Invalid request cursor")?;
            let timestamp = timestamp.parse::<i64>().context("Invalid request cursor")?;
            let seq = seq.parse::<i64>().context("Invalid request cursor")?;
            scope.push_str(" AND (r.timestamp,r.seq)<(?,?)");
            values.extend([timestamp.into(), seq.into()]);
        }
        let mut rows = Self::request_summaries_raw(conn, &scope, &values, 101)?;
        let more = rows.len() > 100;
        rows.truncate(100);
        let next = if more {
            rows.last()
                .and_then(|row| row["_cursor"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        for row in &mut rows {
            row.as_object_mut().unwrap().remove("_cursor");
        }
        Ok((rows, next))
    }

    fn request_summaries(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut rows = Self::request_summaries_raw(conn, predicate, args, limit)?;
        for row in &mut rows {
            row.as_object_mut().unwrap().remove("_cursor");
        }
        Ok(rows)
    }

    fn request_summaries_raw(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        limit: usize,
    ) -> Result<Vec<Value>> {
        // For broad searches, walk newest-first and stop at the feed limit,
        // instead of fetching/sorting every matching request's saved JSON.
        // Tiny selections retain direct rowid lookup, even for very old rows.
        let broad_selection = predicate == DASHBOARD_SELECTION && conn.query_row(
            &format!("SELECT count(*)>{limit} FROM (SELECT 1 FROM temp.dashboard_request_ids LIMIT {})", limit + 1),
            [], |row| row.get::<_, bool>(0),
        )?;
        let scan = if broad_selection {
            "requests r INDEXED BY request_time"
        } else {
            "requests r"
        };
        let mut stmt = conn.prepare(&format!("SELECT id,json_extract(data,'$.timestamp'),source,NULLIF(model,''),status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample,event_kind,json_extract(data,'$.imported_at'),json_extract(data,'$.transport_error'),coalesce({FAILED},0),timestamp,seq FROM {scan} WHERE {predicate} ORDER BY timestamp DESC,seq DESC LIMIT {limit}"))?;
        Ok(stmt.query_map(params_from_iter(args), |r| Ok(json!({"id":r.get::<_,String>(0)?,"timestamp":r.get::<_,Option<i64>>(1)?,"source":r.get::<_,String>(2)?,"model":r.get::<_,Option<String>>(3)?,"status":r.get::<_,Option<i64>>(4)?,"duration_ms":r.get::<_,Option<f64>>(5)?,"input_tokens":r.get::<_,Option<i64>>(6)?,"output_tokens":r.get::<_,Option<i64>>(7)?,"cost_usd":r.get::<_,Option<f64>>(8)?,"cost_basis":r.get::<_,String>(9)?,"answer_count":r.get::<_,i64>(10)?,"capture_complete":r.get::<_,bool>(11)?,"sample":r.get::<_,bool>(12)?,"event_kind":r.get::<_,String>(13)?,"imported_at":r.get::<_,Option<i64>>(14)?,"transport_error":r.get::<_,Option<String>>(15)?,"failed":r.get::<_,bool>(16)?,"_cursor":format!("{}:{}",r.get::<_,i64>(17)?,r.get::<_,i64>(18)?)})))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Expensive answer-key/group filters select parent requests once per
    /// snapshot. Reuse their integer row IDs for every aggregate and feed query.
    fn dashboard_predicate(conn: &Connection, filter: &Filter) -> Result<(String, Vec<SqlValue>)> {
        // Broad searches resolve answer-key matches in one covering scan;
        // narrow scopes keep per-parent lookups rather than scanning history.
        // Export/detail queries keep their existing paged/correlated strategy.
        let mut batch_search = false;
        let has_group = filter.group.as_ref().is_some_and(|group| !group.is_empty());
        if !has_group && filter.search.as_ref().is_some_and(|s| !s.is_empty()) {
            let mut scope = filter.clone();
            scope.search = None;
            let (predicate, args) = Self::predicate(&scope)?;
            let records: i64 =
                conn.query_row("SELECT count(*) FROM requests", [], |row| row.get(0))?;
            let threshold = (records / 4).max(512);
            batch_search = conn.query_row(
                &format!("SELECT count(*)>{threshold} FROM (SELECT 1 FROM requests r WHERE {predicate} LIMIT {})", threshold + 1),
                params_from_iter(&args), |row| row.get(0),
            )?;
        }
        let (predicate, args) = Self::request_predicate(filter, batch_search)?;
        if !has_group
            && filter
                .search
                .as_ref()
                .is_none_or(|search| search.is_empty())
        {
            return Ok((predicate, args));
        }
        // The main database remains read-only. Keep only integer IDs in a
        // connection-local temporary table, with a small pager cache target.
        conn.execute_batch(
            "PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-2048;
             CREATE TEMP TABLE dashboard_request_ids(seq INTEGER PRIMARY KEY);",
        )?;
        // All predicate fields live in these indexes. Reading the request body
        // table here would pull large saved JSON pages into every search poll.
        let index = if filter.window.as_deref() == Some("all") {
            "request_join_scope"
        } else {
            "request_dashboard"
        };
        conn.execute(
            &format!("INSERT INTO temp.dashboard_request_ids SELECT r.seq FROM requests r INDEXED BY {index} WHERE {predicate}"),
            params_from_iter(&args),
        )?;
        Ok((DASHBOARD_SELECTION.into(), Vec::new()))
    }

    fn timeline_spec(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<TimelineSpec> {
        let now = filter
            .as_of
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let custom = filter.from.is_some() || filter.to.is_some();
        let period = match filter.window.as_deref().unwrap_or("24h") {
            "1h" => Some(3_600_000_i64),
            "24h" => Some(86_400_000),
            "7d" => Some(604_800_000),
            _ => None,
        };
        let (start, end) = if let Some(period) = period.filter(|_| !custom) {
            (now.saturating_sub(period), now)
        } else {
            let (first, last): (Option<i64>, Option<i64>) = conn.query_row(
                &format!("SELECT min(timestamp),max(timestamp) FROM requests r INDEXED BY request_dashboard WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL"),
                params_from_iter(args), |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let end = filter.to.or(last).unwrap_or(now);
            let start = filter
                .from
                .or(first)
                .unwrap_or_else(|| end.saturating_sub(86_400_000));
            (start, end.max(start))
        };
        let preferred = match (custom, filter.window.as_deref().unwrap_or("24h")) {
            (false, "24h") => 1_800_000_i128,
            (false, "7d") => 3_600_000,
            _ => 60_000,
        };
        let span = i128::from(end) - i128::from(start) + 1;
        let required = (span + 167) / 168;
        let width = (((required.max(preferred) + preferred - 1) / preferred) * preferred)
            .min(i128::from(i64::MAX)) as i64;
        let count = ((span + i128::from(width) - 1) / i128::from(width)) as usize;
        Ok(TimelineSpec {
            start,
            end,
            width,
            count,
        })
    }

    fn timeline(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<(Vec<Value>, Value)> {
        let spec = Self::timeline_spec(conn, predicate, args, filter)?;
        let mut buckets = BTreeMap::<i64, TimelineBucket>::new();
        let mut stmt = conn.prepare(&format!(
            "SELECT timestamp,{FAILED},duration_ms,cost_usd FROM requests r INDEXED BY request_dashboard WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL"
        ))?;
        let mut rows = stmt.query(params_from_iter(args))?;
        while let Some(row) = rows.next()? {
            let bucket = buckets.entry(spec.bucket(row.get(0)?)).or_default();
            bucket.requests += 1;
            bucket.errors += i64::from(row.get::<_, Option<bool>>(1)?.unwrap_or(false));
            if let Some(value) = row.get::<_, Option<f64>>(2)? {
                bucket.latency.add(value);
            }
            if let Some(value) = row.get::<_, Option<f64>>(3)? {
                bucket.cost.add(value);
            }
        }
        Ok((spec.finish(buckets), spec.metadata()))
    }

    fn group_metadata(conn: &Connection, id: &str) -> Result<Option<Value>> {
        let meta = conn.query_row("SELECT id,key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter FROM groups WHERE id=? OR family_id=? ORDER BY id LIMIT 1",params![id,id], |r| Ok(json!({"id":id,"key":r.get::<_,String>(1)?,"name":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"source":r.get::<_,String>(3)?,"definition_id":r.get::<_,String>(4)?,"presentation_id":r.get::<_,String>(5)?,"definition":parse(r.get(6)?)?,"task_version":r.get::<_,Option<String>>(7)?,"family_id":r.get::<_,Option<String>>(8)?,"family_name":r.get::<_,Option<String>>(9)?,"adapter":r.get::<_,Option<String>>(10)?}))).optional()?;
        let Some(mut result) = meta else {
            return Ok(None);
        };
        let family = result["family_id"].as_str() == Some(id);
        if family {
            result["name"] = result["family_name"].clone();
        }
        result["is_family"] = json!(family);
        let version_count: i64 = conn.query_row(
            "SELECT count(*) FROM groups WHERE source=? AND ((?=1 AND family_id=?) OR (?=0 AND key=?))",
            params![text(&result, "source"),family,id,family,text(&result, "key")],
            |r| r.get(0),
        )?;
        result["version_count"] = json!(version_count);
        Ok(Some(result))
    }

    fn group_summary(conn: &Connection, id: &str, filter: &Filter) -> Result<Option<Value>> {
        let Some(mut result) = Self::group_metadata(conn, id)? else {
            return Ok(None);
        };
        let family = result["is_family"].as_bool().unwrap_or(false);
        let (predicate, mut args) = Self::predicate(filter)?;
        let selector = if family { "g.family_id=?" } else { "g.id=?" };
        args.push(id.to_owned().into());
        let from = format!(
            "FROM answers a JOIN requests r ON r.id=a.request_id JOIN groups g ON g.id=a.group_id WHERE {predicate} AND {selector}"
        );
        let (count,valid,requests,last,mean,confidence): (i64,i64,i64,Option<i64>,Option<f64>,Option<f64>) = conn.query_row(&format!("SELECT count(*),coalesce(sum(a.valid),0),count(DISTINCT r.id),max(r.timestamp),avg(CASE WHEN a.valid THEN a.value_num END),avg(CASE WHEN a.valid THEN a.confidence END) {from}"), params_from_iter(&args), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        result["answer_count"] = json!(count);
        result["valid_count"] = json!(valid);
        result["request_count"] = json!(requests);
        result["last_seen"] = json!(last);
        result["mean_value"] = json!(mean);
        result["mean_confidence"] = json!(confidence);
        result
            .as_object_mut()
            .unwrap()
            .extend(Self::group_metrics(conn, id, family, filter)?);
        let kind = text(&result, "kind");
        let expression = match kind.as_str() {
            "choice" => "a.value_text",
            "noul" => "min(9,CAST(a.value_num*10 AS INTEGER))",
            _ => "CAST(a.value_num AS INTEGER)",
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT CAST({expression} AS TEXT),count(*) {from} AND a.valid=1 GROUP BY 1 ORDER BY {expression}"
        ))?;
        let distribution = stmt
            .query_map(params_from_iter(&args), |r| {
                let raw: String = r.get(0)?;
                Ok(json!({"label":distribution_label(&result, &raw),"count":r.get::<_,i64>(1)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result["distribution"] = json!(distribution);
        Ok(Some(result))
    }

    fn group_metrics(
        conn: &Connection,
        id: &str,
        family: bool,
        filter: &Filter,
    ) -> Result<serde_json::Map<String, Value>> {
        let (predicate, mut args) = Self::predicate(filter)?;
        args.push(id.to_owned().into());
        let selector = if family { "g.family_id=?" } else { "g.id=?" };
        let (parent_predicate, mut parent_args) = Self::predicate(filter)?;
        parent_args.push(id.to_owned().into());
        let parent_selector = if family { "gg.family_id=?" } else { "gg.id=?" };
        let parent_scope = format!(
            "{parent_predicate} AND r.id IN (SELECT aa.request_id FROM answers aa JOIN groups gg ON gg.id=aa.group_id WHERE {parent_selector})"
        );
        let metrics: Value = conn.query_row(
            &format!("SELECT avg(duration_ms),sum(cost_usd),count(cost_usd),sum(input_tokens),sum(output_tokens),coalesce(sum({FAILED}),0) FROM requests r WHERE {parent_scope}"),
            params_from_iter(&parent_args), |row| Ok(json!({
                "mean_latency_ms":row.get::<_,Option<f64>>(0)?, "cost_usd":row.get::<_,Option<f64>>(1)?,
                "cost_known_requests":row.get::<_,i64>(2)?, "input_tokens":row.get::<_,Option<i64>>(3)?,
                "output_tokens":row.get::<_,Option<i64>>(4)?, "error_count":row.get::<_,i64>(5)?
            })),
        )?;
        // Labels are unique by (request_id,key), so this join preserves answer
        // multiplicity while computing reviews and warnings in the same pass.
        let answer_metrics: Value = conn.query_row(
            &format!("SELECT coalesce(sum(coalesce(json_array_length(json_extract(a.data,'$.warnings')),0)>0),0),coalesce(sum(l.label='correct'),0),coalesce(sum(l.label='incorrect'),0),coalesce(sum(l.label='unknown'),0),coalesce(sum(l.label IS NULL),0) FROM answers a JOIN requests r ON r.id=a.request_id JOIN groups g ON g.id=a.group_id LEFT JOIN labels l ON l.request_id=a.request_id AND l.key=a.key WHERE {predicate} AND {selector}"),
            params_from_iter(&args), |row| Ok(json!({
                "warning_count":row.get::<_,i64>(0)?,
                "review_counts":{
                    "correct":row.get::<_,i64>(1)?,"incorrect":row.get::<_,i64>(2)?,
                    "unknown":row.get::<_,i64>(3)?,"unlabeled":row.get::<_,i64>(4)?
                }
            })),
        )?;
        let mut result = metrics.as_object().unwrap().clone();
        result.extend(answer_metrics.as_object().unwrap().clone());
        Ok(result)
    }

    /// Scan compact request metrics once for every overview component. The
    /// covering index avoids reading/parsing saved JSON bodies. All values and
    /// parent membership come from this transaction, including imported events.
    fn dashboard_requests(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<DashboardRequests> {
        let spec = Self::timeline_spec(conn, predicate, args, filter)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id,timestamp,event_kind,answer_count,status,input_tokens,output_tokens,cost_usd,sample,capture_complete,duration_ms,json_extract(data,'$.timestamp'),json_extract(data,'$.transport_error') FROM requests r INDEXED BY request_dashboard WHERE {predicate}"
        ))?;
        let mut rows = stmt.query(params_from_iter(args))?;
        let mut parents = HashMap::new();
        let mut buckets = BTreeMap::<i64, TimelineBucket>::new();
        let mut durations = Vec::new();
        let (mut count, mut answers, mut errors, mut actions, mut samples, mut incomplete) =
            (0_i64, 0_i64, 0_i64, 0_i64, 0_i64, 0_i64);
        let (mut input, mut output) = (None, None);
        let mut cost = Mean::default();
        while let Some(row) = rows.next()? {
            let timestamp: i64 = row.get(1)?;
            parents.insert(row.get::<_, String>(0)?, timestamp);
            if row.get_ref(2)?.as_str()? != "request" {
                actions += 1;
                continue;
            }
            count += 1;
            answers = answers
                .checked_add(row.get(3)?)
                .context("integer overflow")?;
            let error = row
                .get::<_, Option<i64>>(4)?
                .is_some_and(|status| status >= 400)
                || row.get_ref(12)? != rusqlite::types::ValueRef::Null;
            errors += i64::from(error);
            add_integer_sum(&mut input, row.get(5)?)?;
            add_integer_sum(&mut output, row.get(6)?)?;
            let charge: Option<f64> = row.get(7)?;
            if let Some(charge) = charge {
                cost.add(charge);
            }
            samples += i64::from(row.get::<_, bool>(8)?);
            incomplete += i64::from(!row.get::<_, bool>(9)?);
            let duration: Option<f64> = row.get(10)?;
            if let Some(duration) = duration {
                durations.push(duration);
            }
            // Storage timestamp may be imported_at. It is not evidence of an
            // original event time, so unknown timestamps never enter timelines.
            if row.get_ref(11)? != rusqlite::types::ValueRef::Null {
                let bucket = buckets.entry(spec.bucket(timestamp)).or_default();
                bucket.requests += 1;
                bucket.errors += i64::from(error);
                if let Some(duration) = duration {
                    bucket.latency.add(duration);
                }
                if let Some(charge) = charge {
                    bucket.cost.add(charge);
                }
            }
        }
        let mut summary = json!({
            "request_count":count,"answer_count":answers,"error_count":errors,
            "input_tokens":input,"output_tokens":output,"cost_usd":cost.total(),
            "cost_known_requests":cost.count,"sample_count":samples,"incomplete_count":incomplete,
            "action_count":actions
        });
        // Exact nearest-rank percentiles need two selections, not a full sort.
        for (name, p) in [("p50_ms", 0.50), ("p95_ms", 0.95)] {
            summary[name] = if durations.is_empty() {
                Value::Null
            } else {
                let rank = ((durations.len() as f64 * p).ceil() as usize).saturating_sub(1);
                let (_, value, _) = durations.select_nth_unstable_by(rank, f64::total_cmp);
                json!(*value)
            };
        }
        let timeline = spec.finish(buckets);
        Ok(DashboardRequests {
            summary,
            timeline,
            parents,
            timeline_meta: spec.metadata(),
        })
    }

    /// Read each strict group's answers once, accumulating both its metrics and
    /// bins. Ordering by parent ID makes distinct-request counting constant-space.
    fn strict_dashboard_groups(
        conn: &Connection,
        parents: &HashMap<String, i64>,
        filter: &Filter,
    ) -> Result<Vec<(String, Value)>> {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = conn.prepare("SELECT id,kind FROM groups WHERE family_id IS NULL AND instr(lower(key || ' ' || source || ' ' || kind || ' ' || id || ' ' || coalesce(task_version,'')),lower(?))>0 ORDER BY id")?;
        let after = Self::group_cursor(filter)?;
        let mut answers = conn.prepare(
            "SELECT request_id,valid,value_num,confidence,value_text FROM answers INDEXED BY answer_statistics WHERE group_id=? ORDER BY request_id",
        )?;
        let mut selected: Vec<(String, Value)> = Vec::new();
        for row in ids.query_map([filter.group_search.as_deref().unwrap_or("")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, kind) = row?;
            let mut rows = answers.query([&id])?;
            let (mut count, mut valid, mut requests) = (0_i64, 0_i64, 0_i64);
            let (mut values, mut confidence) = (Mean::default(), Mean::default());
            let mut last = i64::MIN;
            let mut previous_request = "";
            let mut numeric_bins = std::collections::BTreeMap::<i64, i64>::new();
            let mut choice_bins = std::collections::BTreeMap::<String, i64>::new();
            while let Some(row) = rows.next()? {
                let Some((request, &timestamp)) = parents.get_key_value(row.get_ref(0)?.as_str()?)
                else {
                    continue;
                };
                count += 1;
                if count == 1 || request != previous_request {
                    requests += 1;
                    previous_request = request;
                }
                last = last.max(timestamp);
                if !row.get::<_, bool>(1)? {
                    continue;
                }
                valid += 1;
                let number: Option<f64> = row.get(2)?;
                if let Some(number) = number {
                    values.add(number);
                }
                if let Some(value) = row.get::<_, Option<f64>>(3)? {
                    confidence.add(value);
                }
                if kind == "choice" {
                    let bin = row.get_ref(4)?.as_str()?;
                    if let Some(count) = choice_bins.get_mut(bin) {
                        *count += 1;
                    } else {
                        choice_bins.insert(bin.to_owned(), 1);
                    }
                } else {
                    let number = number.context("Valid numeric value is missing")?;
                    let bin = if kind == "noul" {
                        ((number * 10.0) as i64).min(9)
                    } else {
                        number as i64
                    };
                    *numeric_bins.entry(bin).or_default() += 1;
                }
            }
            if count == 0
                || after.as_ref().is_some_and(|(time, cursor_id)| {
                    last > *time || (last == *time && id <= *cursor_id)
                })
            {
                continue;
            }
            let bins: Vec<_> = if kind == "choice" {
                choice_bins.into_iter().collect()
            } else {
                numeric_bins
                    .into_iter()
                    .map(|(bin, count)| (bin.to_string(), count))
                    .collect()
            };
            let stats = json!({
                "answer_count":count,"valid_count":valid,"request_count":requests,"last_seen":last,
                "mean_value":values.value(),
                "mean_confidence":confidence.value(),
                "distribution":bins.into_iter().map(|(bin,count)| json!({"label":bin,"count":count})).collect::<Vec<_>>()
            });
            // Retain one page plus lookahead, even for high-cardinality data.
            let position = selected.partition_point(|(other_id, other)| {
                other["last_seen"].as_i64().unwrap() > last
                    || (other["last_seen"] == last && other_id < &id)
            });
            if position < 101 {
                selected.insert(position, (id, stats));
                selected.truncate(101);
            }
        }
        Ok(selected)
    }

    fn group_cursor(filter: &Filter) -> Result<Option<(i64, String)>> {
        filter
            .group_cursor
            .as_deref()
            .map(|cursor| {
                let (timestamp, id) = cursor.split_once(':').context("Invalid group cursor")?;
                if id.is_empty() {
                    bail!("Invalid group cursor");
                }
                Ok((
                    timestamp.parse::<i64>().context("Invalid group cursor")?,
                    id.to_owned(),
                ))
            })
            .transpose()
    }

    pub fn dashboard(&self, filter: &Filter) -> Result<Value> {
        let mut anchored = filter.clone();
        anchored
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let filter = &anchored;
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        let (predicate, args) = Self::dashboard_predicate(&tx, filter)?;
        let DashboardRequests {
            summary,
            timeline,
            parents,
            timeline_meta,
        } = Self::dashboard_requests(&tx, &predicate, &args, filter)?;
        let (requests, feed_next_cursor) =
            Self::request_page(&tx, &predicate, &args, filter.request_cursor.as_deref())?;
        // Families may contain multiple keys per request. Keep their exact SQL
        // distinct counts; strict groups compute metrics and bins in one pass.
        let from = format!(
            "FROM groups g INDEXED BY sqlite_autoindex_groups_1 CROSS JOIN answers a INDEXED BY answer_statistics ON a.group_id=g.id CROSS JOIN requests r INDEXED BY request_join_scope ON r.id=a.request_id WHERE {predicate}"
        );
        let metrics = "count(*) AS answer_count,coalesce(sum(a.valid),0) AS valid_count,count(DISTINCT r.id) AS request_count,max(r.timestamp) AS last_seen,avg(CASE WHEN a.valid THEN a.value_num END) AS mean_value,avg(CASE WHEN a.valid THEN a.confidence END) AS mean_confidence";
        let mut stats = Self::strict_dashboard_groups(&tx, &parents, filter)?;
        let mut family_args = args.clone();
        let mut family_scope = String::new();
        if let Some(search) = filter
            .group_search
            .as_ref()
            .filter(|search| !search.is_empty())
        {
            family_scope.push_str(" AND instr(lower(coalesce(g.family_name,'') || ' ' || g.source || ' ' || g.kind || ' ' || g.family_id),lower(?))>0");
            family_args.push(search.clone().into());
        }
        let mut family_after = String::new();
        if let Some((timestamp, id)) = Self::group_cursor(filter)? {
            family_after.push_str(" HAVING last_seen<? OR (last_seen=? AND display_id>?)");
            family_args.extend([timestamp.into(), timestamp.into(), id.into()]);
        }
        let mut stmt = tx.prepare(&format!("SELECT g.family_id AS display_id,{metrics} {from} AND g.family_id IS NOT NULL {family_scope} GROUP BY g.family_id {family_after} ORDER BY last_seen DESC,display_id LIMIT 101"))?;
        let family_stats = stmt.query_map(params_from_iter(&family_args), |r| Ok((r.get::<_,String>(0)?, json!({
            "answer_count":r.get::<_,i64>(1)?,"valid_count":r.get::<_,i64>(2)?,"request_count":r.get::<_,i64>(3)?,
            "last_seen":r.get::<_,Option<i64>>(4)?,"mean_value":r.get::<_,Option<f64>>(5)?,"mean_confidence":r.get::<_,Option<f64>>(6)?,"distribution":[]
        }))))?.collect::<rusqlite::Result<Vec<_>>>()?;
        stats.extend(family_stats);
        stats.sort_by(|(left_id, left), (right_id, right)| {
            right["last_seen"]
                .as_i64()
                .cmp(&left["last_seen"].as_i64())
                .then_with(|| left_id.cmp(right_id))
        });
        let more_groups = stats.len() > 100;
        stats.truncate(100);
        let group_next_cursor = if more_groups {
            stats
                .last()
                .map(|(id, stats)| format!("{}:{id}", stats["last_seen"].as_i64().unwrap()))
        } else {
            None
        };
        let mut groups = Vec::new();
        for (id, stats) in &stats {
            if let Some(mut group) = Self::group_metadata(&tx, id)? {
                group
                    .as_object_mut()
                    .unwrap()
                    .extend(stats.as_object().unwrap().clone());
                let distribution: Vec<_> = group["distribution"].as_array().unwrap().iter().map(|bin| {
                    json!({"label":distribution_label(&group, bin["label"].as_str().unwrap()),"count":bin["count"]})
                }).collect();
                group["distribution"] = json!(distribution);
                // Comparison-only metrics belong to the detail endpoint. They
                // read saved answer JSON and must not run for every group on
                // each overview poll when the overview does not display them.
                groups.push(group);
            }
        }
        let families: Vec<_> = groups.iter().filter(|g| g["is_family"] == true).collect();
        if !families.is_empty() {
            let placeholders = vec!["?"; families.len()].join(",");
            let mut distribution_args = args.clone();
            distribution_args.extend(families.iter().map(|g| SqlValue::from(text(g, "id"))));
            let bin = "CASE a.kind WHEN 'choice' THEN a.value_text WHEN 'noul' THEN CAST(min(9,CAST(a.value_num*10 AS INTEGER)) AS TEXT) ELSE CAST(CAST(a.value_num AS INTEGER) AS TEXT) END";
            let mut stmt = tx.prepare(&format!("SELECT display_id,bin,bin_count FROM (SELECT g.family_id AS display_id,{bin} AS bin,count(*) AS bin_count,a.kind AS kind {from} AND a.valid=1 AND g.family_id IN ({placeholders}) GROUP BY g.family_id,bin) ORDER BY display_id,CASE WHEN kind='choice' THEN bin END,CASE WHEN kind<>'choice' THEN CAST(bin AS INTEGER) END"))?;
            let bins = stmt.query_map(params_from_iter(&distribution_args), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            let positions: std::collections::HashMap<String, usize> = groups
                .iter()
                .enumerate()
                .map(|(i, g)| (text(g, "id"), i))
                .collect();
            for bin in bins {
                let (id, raw, count) = bin?;
                if let Some(index) = positions.get(&id) {
                    let group = &mut groups[*index];
                    let label = distribution_label(group, &raw);
                    group["distribution"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"label":label,"count":count}));
                }
            }
        }
        let mut sources =
            tx.prepare("SELECT DISTINCT source FROM requests ORDER BY source LIMIT 1000")?;
        let sources = sources
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut models = tx.prepare(
            "SELECT DISTINCT model FROM requests WHERE model<>'' ORDER BY model LIMIT 1000",
        )?;
        let models = models
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"generated_at":filter.as_of,"sample":summary["sample_count"].as_i64().unwrap_or(0)>0,"summary":summary,"timeline":timeline,"timeline_meta":timeline_meta,"groups":groups,"requests":requests,"sources":sources,"models":models,"health":{},"feed_limit":100,"group_limit":100,"feed_next_cursor":feed_next_cursor,"group_next_cursor":group_next_cursor}),
        )
    }

    // A record spans parent, answers and labels. Requiring a transaction keeps
    // retention/deletion or review edits from mixing different read snapshots.
    fn request_parent(conn: &rusqlite::Transaction<'_>, id: &str) -> Result<Option<(Value, i64)>> {
        let record = conn
            .query_row(
                "SELECT data,answer_count FROM requests WHERE id=?",
                [id],
                |r| Ok((parse(r.get(0)?)?, r.get(1)?)),
            )
            .optional()?;
        #[cfg(test)]
        if record.is_some() {
            tests::after_parent_read(id);
        }
        Ok(record)
    }

    fn request_from(conn: &rusqlite::Transaction<'_>, id: &str) -> Result<Option<Value>> {
        let Some((mut record, _)) = Self::request_parent(conn, id)? else {
            return Ok(None);
        };
        let mut stmt=conn.prepare("SELECT a.data,g.definition FROM answers a JOIN groups g ON g.id=a.group_id WHERE request_id=? ORDER BY a.rowid")?;
        let answers = stmt
            .query_map([id], |r| {
                let mut value = parse(r.get(0)?)?;
                value["definition"] = parse(r.get(1)?)?;
                Ok(value)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record["answers"] = json!(answers);
        let mut stmt = conn.prepare(
            "SELECT key,label,timestamp,source FROM labels WHERE request_id=? ORDER BY key",
        )?;
        let labels=stmt.query_map([id],|r|Ok(json!({"key":r.get::<_,String>(0)?,"label":r.get::<_,String>(1)?,"timestamp":r.get::<_,i64>(2)?,"source":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        record["labels"] = json!(labels);
        record["failed"] = json!(
            record["status"]
                .as_i64()
                .is_some_and(|status| status >= 400)
                || !record["transport_error"].is_null()
        );
        Ok(Some(record))
    }

    pub fn request(&self, id: &str) -> Result<Option<Value>> {
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        Self::request_from(&tx, id)
    }

    pub fn group(&self, id: &str, filter: &Filter) -> Result<Option<Value>> {
        let mut anchored = filter.clone();
        anchored
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let filter = &anchored;
        let mut connection = self.reader()?;
        let conn = connection.transaction()?;
        let Some(group) = Self::group_summary(&conn, id, filter)? else {
            return Ok(None);
        };
        // The requested group narrows the caller's parent scope. Replacing an
        // existing group filter would make feeds and observations disagree
        // with the aggregate above when parents contain multiple groups.
        let (mut predicate, mut args) = Self::predicate(filter)?;
        if filter.group.as_deref() != Some(id) {
            predicate.push_str(" AND EXISTS(SELECT 1 FROM answers ga JOIN groups gg ON gg.id=ga.group_id WHERE ga.request_id=r.id AND (gg.id=? OR gg.family_id=?))");
            args.push(id.to_owned().into());
            args.push(id.to_owned().into());
        }
        let requests = Self::request_summaries(&conn, &predicate, &args, 100)?;
        let (timeline, timeline_meta) = Self::timeline(&conn, &predicate, &args, filter)?;
        let family = group["is_family"].as_bool().unwrap_or(false);
        let (version_scope, mut version_args) = Self::predicate(filter)?;
        version_args.extend([
            text(&group, "source").into(),
            (family as i64).into(),
            id.to_owned().into(),
            (family as i64).into(),
            text(&group, "key").into(),
            id.to_owned().into(),
        ]);
        // Keep the viewed version in the bounded comparison list, then prefer
        // versions with the most recent activity in the caller's parent scope.
        let mut stmt = conn.prepare(&format!("SELECT g.id FROM groups g LEFT JOIN answers a ON a.group_id=g.id LEFT JOIN requests r ON r.id=a.request_id AND {version_scope} WHERE g.source=? AND ((?=1 AND g.family_id=?) OR (?=0 AND g.key=?)) GROUP BY g.id ORDER BY (g.id=?) DESC,max(r.timestamp) DESC,g.id LIMIT 100"))?;
        let ids = stmt
            .query_map(params_from_iter(&version_args), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut versions = Vec::new();
        for id in ids {
            if group["id"] == id {
                versions.push(group.clone());
                continue;
            }
            if let Some(version) = Self::group_summary(&conn, &id, filter)? {
                versions.push(version);
            }
        }
        let mut answer_args = args.clone();
        answer_args.push(id.to_owned().into());
        answer_args.push(id.to_owned().into());
        let mut stmt=conn.prepare(&format!("SELECT a.request_id,json_extract(r.data,'$.timestamp'),r.source,a.key,a.data,l.label,json_extract(r.data,'$.imported_at') FROM answers a JOIN requests r ON r.id=a.request_id JOIN groups g ON g.id=a.group_id LEFT JOIN labels l ON l.request_id=a.request_id AND l.key=a.key WHERE {predicate} AND (g.id=? OR g.family_id=?) ORDER BY r.timestamp DESC,r.seq DESC,a.rowid LIMIT 100"))?;
        let answers = stmt
            .query_map(params_from_iter(&answer_args), |r| {
                let mut a = parse(r.get(4)?)?;
                a["request_id"] = json!(r.get::<_, String>(0)?);
                a["timestamp"] = json!(r.get::<_, Option<i64>>(1)?);
                a["source"] = json!(r.get::<_, String>(2)?);
                a["label"] = json!(r.get::<_, Option<String>>(5)?);
                a["imported_at"] = json!(r.get::<_, Option<i64>>(6)?);
                Ok(a)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(
            json!({"total_answers":group["answer_count"],"group":group,"versions":versions,"timeline":timeline,"timeline_meta":timeline_meta,"requests":requests,"answers":answers,"detail_limit":100}),
        ))
    }

    /// A short read transaction per page prevents slow downloads from pinning
    /// the WAL. `through` freezes the sequence ceiling while new calls arrive.
    pub fn export_page(
        &self,
        filter: &Filter,
        format: &str,
        after: i64,
        through: Option<i64>,
    ) -> Result<(String, i64, i64, bool)> {
        if !["jsonl", "csv"].contains(&format) {
            bail!("Export format must be jsonl or csv");
        }
        let mut connection = self.reader()?;
        let conn = connection.transaction()?;
        let through = match through {
            Some(value) => value,
            None => conn.query_row("SELECT coalesce(max(seq),0) FROM requests", [], |r| {
                r.get(0)
            })?,
        };
        let (predicate, mut args) = Self::predicate(filter)?;
        args.push(after.into());
        args.push(through.into());
        let mut stmt = conn.prepare(&format!("SELECT id,seq FROM requests r WHERE {predicate} AND seq>? AND seq<=? ORDER BY seq LIMIT 100"))?;
        let ids = stmt
            .query_map(params_from_iter(&args), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let full_page = ids.len() == 100;
        let mut out = String::new();
        if format == "csv" && after == 0 {
            out.push_str("id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,event_kind\n");
        }
        let mut cursor = after;
        for (id, seq) in ids {
            if out.len() >= 2 * 1024 * 1024 {
                return Ok((out, cursor, through, true));
            }
            if format == "jsonl" {
                if let Some(record) = Self::request_from(&conn, &id)? {
                    out.push_str(&serde_json::to_string(&record)?);
                    out.push('\n');
                }
            } else if let Some((record, answer_count)) = Self::request_parent(&conn, &id)? {
                // CSV contains parent metrics only. Its saved answer count is
                // committed atomically with the children, so exporting it does
                // not require loading every answer, definition and review.
                let fields = [
                    json!(id),
                    record["timestamp"].clone(),
                    record["source"].clone(),
                    record["model"].clone(),
                    record["status"].clone(),
                    record["duration_ms"].clone(),
                    record["input_tokens"].clone(),
                    record["output_tokens"].clone(),
                    record["cost_usd"].clone(),
                    record["cost_basis"].clone(),
                    json!(answer_count),
                    json!(record["event_kind"].as_str().unwrap_or("request")),
                ];
                out.push_str(&fields.iter().map(csv_cell).collect::<Vec<_>>().join(","));
                out.push('\n');
            }
            if out.len() > 32 * 1024 * 1024 {
                bail!("An export record exceeds the 32 MiB page limit");
            }
            cursor = seq;
        }
        Ok((out, cursor, through, full_page))
    }

    #[cfg(test)]
    pub fn export(&self, filter: &Filter, format: &str) -> Result<String> {
        let mut filter = filter.clone();
        filter
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let mut output = String::new();
        let mut after = 0;
        let mut ceiling = None;
        loop {
            let (page, cursor, through, more) =
                self.export_page(&filter, format, after, ceiling)?;
            output.push_str(&page);
            if output.len() > 16 * 1024 * 1024 {
                bail!("Use paginated exports for large histories");
            }
            if !more {
                break;
            }
            after = cursor;
            ceiling = Some(through);
        }
        Ok(output)
    }

    pub fn add_label(&self, id: &str, key: &str, label: &str) -> Result<()> {
        if !["correct", "incorrect", "unknown"].contains(&label) {
            bail!("Label must be correct, incorrect or unknown");
        }
        let conn = self.writer_connection()?;
        let found: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM answers WHERE request_id=? AND key=?)",
            params![id, key],
            |r| r.get(0),
        )?;
        if !found {
            bail!("Answer does not exist");
        }
        conn.execute("INSERT OR REPLACE INTO labels(request_id,key,label,timestamp,source) VALUES(?,?,?,?,?)",params![id,key,label,Utc::now().timestamp_millis(),"local-reviewer"])?;
        Ok(())
    }

    pub fn delete_all(&self) -> Result<()> {
        let mut conn = self.writer_connection()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM requests", [])?;
        tx.execute("DELETE FROM groups", [])?;
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

fn distribution_label(group: &Value, raw: &str) -> String {
    match group["kind"].as_str() {
        Some("noul") => {
            let low = raw.parse::<f64>().unwrap_or(0.0) / 10.0;
            if raw == "9" {
                "0.9–1.0".into()
            } else {
                format!("{low:.1}–<{:.1}", low + 0.1)
            }
        }
        Some("score") => {
            let low = raw.parse::<usize>().unwrap_or(0);
            let maximum = group["definition"]["criteria"]
                .as_array()
                .map(|a| a.len().saturating_sub(1));
            if maximum == Some(low) {
                raw.into()
            } else {
                format!("{low}–<{}", low + 1)
            }
        }
        _ => raw.into(),
    }
}

fn csv_cell(value: &Value) -> String {
    let raw = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    };
    // Prevent spreadsheet formula interpretation without changing JSONL exports.
    let first_visible = raw.trim_start_matches(char::is_whitespace).chars().next();
    let formula = matches!(
        first_visible,
        Some('=' | '+' | '-' | '@' | '＝' | '＋' | '－' | '＠')
    );
    let leading_control = raw.starts_with(['\t', '\r', '\n', '\0']);
    let safe = if formula || leading_control {
        format!("'{raw}")
    } else {
        raw
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    pub(super) fn assert_overview_matches_detail(overview: &Value, detail: &Value) {
        let mut shared = detail.clone();
        for field in [
            "warning_count",
            "review_counts",
            "mean_latency_ms",
            "cost_usd",
            "cost_known_requests",
            "input_tokens",
            "output_tokens",
            "error_count",
        ] {
            assert!(overview.get(field).is_none(), "detail-only field {field}");
            assert!(shared.as_object_mut().unwrap().remove(field).is_some());
        }
        assert_eq!(*overview, shared);
    }

    #[test]
    fn encrypted_history_rejects_wrong_key_and_migrates_existing_records() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let plain = Store::open(&path, 7, 100).unwrap();
        plain
            .write_batch(&mut plain.writer_connection().unwrap(), &[record("before")])
            .unwrap();
        plain.set_credential_approval(Some("digest-only")).unwrap();
        drop(plain);
        let key = "a".repeat(64);
        let encrypted = Store::open_encrypted(&path, key.clone(), 7, 100).unwrap();
        assert!(!plaintext_database(&path).unwrap());
        assert_eq!(
            encrypted.credential_approval().unwrap().as_deref(),
            Some("digest-only")
        );
        let without_key = Connection::open(&path).unwrap();
        assert!(
            without_key
                .query_row("SELECT count(*) FROM requests", [], |row| row
                    .get::<_, i64>(0))
                .is_err()
        );
        assert_eq!(
            encrypted
                .export(
                    &Filter {
                        window: Some("all".into()),
                        ..Filter::default()
                    },
                    "jsonl"
                )
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(Store::open_encrypted(&path, "b".repeat(64), 7, 100).is_err());
        let reopened = Store::open_encrypted(&path, key, 7, 100).unwrap();
        assert_eq!(
            reopened
                .export(
                    &Filter {
                        window: Some("all".into()),
                        ..Filter::default()
                    },
                    "jsonl"
                )
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert!(Store::open_encrypted(&path, "short".into(), 7, 100).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_migration_follows_database_symlink_without_leaving_target_plaintext() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.sqlite");
        let link = directory.path().join("history.sqlite");
        Store::open(&target, 7, 100).unwrap();
        symlink(&target, &link).unwrap();
        Store::open_encrypted(&link, "c".repeat(64), 7, 100).unwrap();
        assert!(!plaintext_database(&target).unwrap());
        assert!(link.is_symlink());
    }

    #[test]
    fn system_credential_approval_is_revocable_without_storing_the_token() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("approval.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        let token = "jo_local_dummy-secret-token";
        let hash = format!("{:x}", sha2::Sha256::digest(token.as_bytes()));
        store.set_credential_approval(Some(&hash)).unwrap();
        assert_eq!(
            store.credential_approval().unwrap().as_deref(),
            Some(hash.as_str())
        );
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(token.len())
                .any(|bytes| bytes == token.as_bytes())
        );
        store.set_credential_approval(None).unwrap();
        assert!(store.credential_approval().unwrap().is_none());
        let reopened = Store::open(&path, 7, 100).unwrap();
        assert!(reopened.credential_approval().unwrap().is_none());
    }
    fn record(id: &str) -> Value {
        json!({"schema_version":1,"id":id,"timestamp":Utc::now().timestamp_millis(),"source":"test","model":"jev-test","status":200,"duration_ms":4.0,"cost_usd":0.1,"cost_basis":"synthetic","input_tokens":100,"output_tokens":3,"capture_complete":true,"sample":true,"answers":[{"key":"urgent","kind":"noul","group_id":"g","definition_id":"d","presentation_id":"p","definition":{"type":"noul","instructions":"Urgent?"},"valid":true,"value":0.8}]})
    }

    #[test]
    fn unsupported_schema_is_rejected_before_changing_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        let conn = store.writer_connection().unwrap();
        conn.execute_batch(
            "CREATE INDEX answer_group ON answers(group_id,request_id);
            UPDATE schema_version SET version=999;
            PRAGMA journal_mode=DELETE;",
        )
        .unwrap();
        drop(conn);
        drop(store);
        let before = std::fs::read(&path).unwrap();
        let error = Store::open(&path, 7, 100)
            .err()
            .expect("Future schema must be rejected");
        assert!(error.to_string().contains("Unsupported database schema"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "Failed startup must not change schema, data, or journal mode"
        );
    }

    #[test]
    fn unsupported_schema_in_crash_left_wal_preserves_database_and_wal_bytes() {
        for encrypted in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let original = directory.path().join("original.sqlite");
            let crashed = directory.path().join("crashed.sqlite");
            let key = "e".repeat(64);
            let store = if encrypted {
                Store::open_encrypted(&original, key.clone(), 7, 100)
            } else {
                Store::open(&original, 7, 100)
            }
            .unwrap();
            let writer = store.writer_connection().unwrap();
            writer
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
            let main = std::fs::read(&original).unwrap();
            writer
                .execute("UPDATE schema_version SET version=999", [])
                .unwrap();
            assert_eq!(
                std::fs::read(&original).unwrap(),
                main,
                "The unsupported version must exist only in the committed WAL"
            );
            let peer = |path: &Path, suffix: &str| {
                let mut name = path.as_os_str().to_owned();
                name.push(suffix);
                PathBuf::from(name)
            };
            // The writer is quiescent. Copying its complete file set leaves a
            // fixture with committed WAL state and no live SQLite connection.
            for suffix in ["", "-wal", "-shm"] {
                std::fs::copy(peer(&original, suffix), peer(&crashed, suffix)).unwrap();
            }
            drop(writer);
            let before: Vec<_> = ["", "-wal"]
                .into_iter()
                .map(|suffix| {
                    let bytes = std::fs::read(peer(&crashed, suffix)).unwrap();
                    assert!(!bytes.is_empty());
                    (suffix, sha2::Sha256::digest(bytes))
                })
                .collect();
            let error = if encrypted {
                Store::open_encrypted(&crashed, key, 7, 100)
            } else {
                Store::open(&crashed, 7, 100)
            }
            .err()
            .expect("Committed future schema must be rejected");
            assert!(
                error
                    .to_string()
                    .contains("Unsupported database schema [999]")
            );
            for (suffix, digest) in before {
                assert_eq!(
                    sha2::Sha256::digest(std::fs::read(peer(&crashed, suffix)).unwrap()),
                    digest,
                    "Rejected startup must preserve {suffix:?} data bytes (encrypted={encrypted})"
                );
            }
            // SHM is a transient WAL index; SQLite may rebuild it while reading
            // a crash-left WAL. Its bytes are not persistent database content.
        }
    }

    #[test]
    fn unsupported_plaintext_and_encrypted_schemas_survive_live_startup_unchanged() {
        for encrypted in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("future.sqlite");
            let key = "d".repeat(64);
            let store = if encrypted {
                Store::open_encrypted(&path, key.clone(), 7, 100).unwrap()
            } else {
                Store::open(&path, 7, 100).unwrap()
            };
            let conn = store.writer_connection().unwrap();
            conn.execute_batch(
                "UPDATE schema_version SET version=999; PRAGMA journal_mode=DELETE;",
            )
            .unwrap();
            drop(conn);
            drop(store);
            let before = std::fs::read(&path).unwrap();
            let error = Store::open_encrypted(&path, key, 7, 100).err().unwrap();
            assert!(error.to_string().contains("Unsupported database schema"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert_eq!(plaintext_database(&path).unwrap(), !encrypted);
        }
    }

    #[test]
    fn dashboard_filters_select_whole_requests_without_cross_query_leakage() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("search.sqlite"), 7, 100).unwrap();
        let mut first = record("first");
        let mut companion = first["answers"][0].clone();
        companion["key"] = json!("followup");
        companion["group_id"] = json!("other-group");
        companion["value"] = json!(0.2);
        first["answers"]
            .as_array_mut()
            .unwrap()
            .push(companion.clone());
        let mut source_match = record("second");
        source_match["source"] = json!("URGENT archive");
        source_match["answers"] = json!([companion.clone()]);
        let mut id_match = record("urgent-id");
        id_match["answers"] = json!([companion.clone()]);
        let mut unmatched = record("fourth");
        unmatched["answers"] = json!([companion]);
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                &[first, source_match, id_match, unmatched],
            )
            .unwrap();

        let cases = [
            (json!({"search":"UrGeNt"}), 3, 4),
            (json!({"group":"g"}), 1, 2),
            (json!({"search":"first", "group":"g"}), 1, 2),
            (json!({"search":"second", "group":"g"}), 0, 0),
            (json!({"search":"%_"}), 0, 0),
            (json!({"search":""}), 4, 5),
            (json!({"search":"UrGeNt"}), 3, 4),
        ];
        for (case, requests, answers) in cases {
            let filter: Filter = serde_json::from_value(case.clone()).unwrap();
            let dashboard = store.dashboard(&filter).unwrap();
            assert_eq!(dashboard["summary"]["request_count"], requests, "{case}");
            assert_eq!(dashboard["summary"]["answer_count"], answers, "{case}");
            let grouped: i64 = dashboard["groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["answer_count"].as_i64().unwrap())
                .sum();
            assert_eq!(grouped, answers, "{case}");
            assert_eq!(
                dashboard["requests"].as_array().unwrap().len(),
                requests as usize,
                "{case}"
            );
        }
        let schema_count: i64 = store
            .reader()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='dashboard_request_ids'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            schema_count, 0,
            "Request selections must not persist in the main database"
        );
    }

    #[test]
    fn empty_group_filter_matches_an_omitted_filter_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("empty-group.sqlite"), 7, 100).unwrap();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[record("one")])
            .unwrap();
        let unfiltered = Filter {
            window: Some("all".into()),
            as_of: Some(Utc::now().timestamp_millis()),
            ..Default::default()
        };
        let empty = Filter {
            group: Some(String::new()),
            ..unfiltered.clone()
        };
        assert_eq!(
            store.dashboard(&empty).unwrap(),
            store.dashboard(&unfiltered).unwrap()
        );
        assert_eq!(
            store.group("g", &empty).unwrap(),
            store.group("g", &unfiltered).unwrap()
        );
        for format in ["jsonl", "csv"] {
            assert_eq!(
                store.export(&empty, format).unwrap(),
                store.export(&unfiltered, format).unwrap()
            );
        }
    }

    #[test]
    fn group_detail_preserves_the_parent_scope_for_every_section() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("intersecting-groups.sqlite"), 7, 100).unwrap();
        let mut shared = record("shared-parent");
        let mut companion = shared["answers"][0].clone();
        companion["key"] = json!("followup");
        companion["group_id"] = json!("other-group");
        companion["family_id"] = json!("other-family");
        companion["family_name"] = json!("Followup family");
        shared["answers"]
            .as_array_mut()
            .unwrap()
            .push(companion.clone());
        let only_original = record("original-only");
        let mut only_companion = record("companion-only");
        only_companion["answers"] = json!([companion]);
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                &[shared, only_original, only_companion],
            )
            .unwrap();
        for (selected, requested, count) in [
            ("g", "other-group", 1),
            ("g", "other-family", 1),
            ("other-family", "g", 1),
            ("absent-group", "g", 0),
        ] {
            let detail = store
                .group(
                    requested,
                    &Filter {
                        window: Some("all".into()),
                        group: Some(selected.into()),
                        ..Default::default()
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(detail["group"]["request_count"], count);
            assert_eq!(detail["total_answers"], count);
            for key in ["requests", "answers"] {
                let rows = detail[key].as_array().unwrap();
                assert_eq!(rows.len(), count, "{requested} within {selected}: {key}");
                if count > 0 {
                    let id = if key == "requests" {
                        "id"
                    } else {
                        "request_id"
                    };
                    assert_eq!(rows[0][id], "shared-parent");
                }
            }
            let timeline_count: i64 = detail["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .map(|bucket| bucket["requests"].as_i64().unwrap())
                .sum();
            assert_eq!(timeline_count, count as i64);
            for version in detail["versions"].as_array().unwrap() {
                assert_eq!(version["request_count"], count);
            }
        }
    }

    #[test]
    fn strict_dashboard_counts_distinct_parents_and_only_valid_known_values() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("strict-groups.sqlite"), 7, 100).unwrap();
        let now = Utc::now().timestamp_millis();
        let mut first = record("parent-a");
        first["timestamp"] = json!(now - 100);
        first["answers"][0]["confidence"] = json!(0.9);
        let mut additional = first["answers"][0].clone();
        additional["key"] = json!("second-key");
        additional["value"] = json!(0.2);
        additional["confidence"] = json!(0.3);
        first["answers"].as_array_mut().unwrap().push(additional);
        let mut invalid = record("parent-b");
        invalid["timestamp"] = json!(now);
        invalid["answers"][0]["valid"] = json!(false);
        invalid["answers"][0]["value"] = json!(1e308);
        invalid["answers"][0]["confidence"] = json!(0.99);
        let mut unknown_confidence = record("parent-c");
        unknown_confidence["timestamp"] = json!(now - 200);
        unknown_confidence["answers"][0]["value"] = json!(0.4);
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                &[first, invalid, unknown_confidence],
            )
            .unwrap();
        let filter = Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let overview = store.dashboard(&filter).unwrap();
        let group = &overview["groups"][0];
        assert_eq!(group["answer_count"], 4);
        assert_eq!(group["valid_count"], 3);
        assert_eq!(group["request_count"], 3);
        assert_eq!(group["last_seen"], now);
        assert_eq!(group["mean_confidence"], 0.6);
        assert_eq!(
            group["distribution"],
            json!([
                {"label":"0.2–<0.3","count":1},
                {"label":"0.4–<0.5","count":1},
                {"label":"0.8–<0.9","count":1}
            ])
        );
        assert_overview_matches_detail(
            group,
            &store.group("g", &filter).unwrap().unwrap()["group"],
        );
    }

    #[test]
    fn dashboard_merges_latest_hundred_strict_groups_and_families_with_stable_ties() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("many-groups.sqlite"), 7, 1000).unwrap();
        let now = Utc::now().timestamp_millis();
        let mut records = Vec::new();
        let mut expected = Vec::new();
        for index in 0..130 {
            let id = format!("strict-{index:03}");
            let timestamp = now - (index % 7) * 100;
            let mut request = record(&format!("strict-parent-{index:03}"));
            request["timestamp"] = json!(timestamp);
            request["answers"][0]["group_id"] = json!(id);
            records.push(request);
            expected.push((id, timestamp));
        }
        for index in 0..15 {
            let family = format!("family-{index:03}");
            let timestamp = now - (index % 7) * 100;
            let mut request = record(&format!("family-parent-{index:03}"));
            request["timestamp"] = json!(timestamp);
            let mut answers = Vec::new();
            for key in 0..2 {
                let mut answer = request["answers"][0].clone();
                answer["key"] = json!(format!("candidate-{key}"));
                answer["group_id"] = json!(format!("member-{index:03}-{key}"));
                answer["family_id"] = json!(family);
                answer["family_name"] = json!("Related questions");
                answer["valid"] = json!(key == 0 || index % 3 != 0);
                answers.push(answer);
            }
            request["answers"] = json!(answers);
            records.push(request);
            expected.push((family, timestamp));
        }
        store
            .write_batch(&mut store.writer_connection().unwrap(), &records)
            .unwrap();
        expected.sort_by(|(left_id, left_time), (right_id, right_time)| {
            right_time
                .cmp(left_time)
                .then_with(|| left_id.cmp(right_id))
        });
        expected.truncate(100);
        let filter = Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let overview = store.dashboard(&filter).unwrap();
        let groups = overview["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 100);
        assert_eq!(overview["group_limit"], 100);
        assert_eq!(
            groups
                .iter()
                .map(|group| text(group, "id"))
                .collect::<Vec<_>>(),
            expected.into_iter().map(|(id, _)| id).collect::<Vec<_>>()
        );
        assert!(groups.iter().any(|group| group["is_family"] == true));
        assert!(groups.iter().any(|group| group["is_family"] == false));
        let connection = store.reader().unwrap();
        for group in groups {
            let baseline = Store::group_summary(&connection, &text(group, "id"), &filter)
                .unwrap()
                .unwrap();
            assert_overview_matches_detail(group, &baseline);
            assert_eq!(group["request_count"], 1);
            if group["is_family"] == true {
                assert_eq!(group["answer_count"], 2);
            }
        }
        let narrowed = Filter {
            window: Some("1h".into()),
            source: Some("test".into()),
            search: Some("strict-parent-129".into()),
            ..Default::default()
        };
        let overview = store.dashboard(&narrowed).unwrap();
        assert_eq!(overview["groups"].as_array().unwrap().len(), 1);
        assert_eq!(overview["groups"][0]["id"], "strict-129");
        assert_overview_matches_detail(
            &overview["groups"][0],
            &Store::group_summary(&connection, "strict-129", &narrowed)
                .unwrap()
                .unwrap(),
        );
        let missing_source = Filter {
            source: Some("absent".into()),
            ..Default::default()
        };
        assert!(
            store.dashboard(&missing_source).unwrap()["groups"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn overview_and_detail_agree_for_mixed_definitions_and_families() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite"), 7, 10_000).unwrap();
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                &crate::model::sample_records(),
            )
            .unwrap();
        let baseline = store
            .dashboard(&Filter {
                window: Some("all".into()),
                ..Default::default()
            })
            .unwrap();
        let family = baseline["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["is_family"] == true)
            .unwrap();
        assert!(
            family["answer_count"].as_i64().unwrap() > family["request_count"].as_i64().unwrap()
        );
        let strict = baseline["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["is_family"] == false)
            .unwrap();
        let cases = [
            json!({"window":"all"}),
            json!({"window":"all","source":"Code search"}),
            json!({"window":"all","source":"Support inbox"}),
            json!({"window":"all","model":"jev-synthetic-b"}),
            json!({"window":"1h"}),
            json!({"window":"24h"}),
            json!({"window":"7d"}),
            json!({"window":"all","status":"error"}),
            json!({"window":"all","search":"urgency"}),
            json!({"window":"all","group":family["id"]}),
            json!({"window":"all","group":strict["id"]}),
            json!({"window":"24h","source":"Code search","model":"jev-synthetic-b","search":"c0"}),
        ];
        for case in cases {
            let mut filter: Filter = serde_json::from_value(case.clone()).unwrap();
            filter.as_of = Some(Utc::now().timestamp_millis());
            let overview = store.dashboard(&filter).unwrap();
            assert!(
                !overview["groups"].as_array().unwrap().is_empty(),
                "empty filter fixture: {case}"
            );
            for group in overview["groups"].as_array().unwrap() {
                let detail = store
                    .group(group["id"].as_str().unwrap(), &filter)
                    .unwrap()
                    .unwrap();
                assert_overview_matches_detail(group, &detail["group"]);
                let total: i64 = group["distribution"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|bin| bin["count"].as_i64().unwrap())
                    .sum();
                assert_eq!(total, group["valid_count"].as_i64().unwrap());
                assert!(
                    group["request_count"].as_i64().unwrap()
                        <= group["answer_count"].as_i64().unwrap()
                );
            }
        }
    }

    #[test]
    fn bounded_windows_use_both_frozen_boundaries_and_all_keeps_future_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("window.sqlite"), 14, 100).unwrap();
        let as_of = Utc::now().timestamp_millis() - 60_000;
        let records: Vec<_> = [
            ("older", as_of - 604_800_001),
            ("week", as_of - 604_800_000),
            ("day", as_of - 86_400_000),
            ("hour", as_of - 3_600_000),
            ("end", as_of),
            ("future", as_of + 1),
        ]
        .into_iter()
        .map(|(id, timestamp)| {
            let mut row = record(id);
            row["timestamp"] = json!(timestamp);
            row
        })
        .collect();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &records)
            .unwrap();
        for (window, count) in [("1h", 2), ("24h", 3), ("7d", 4), ("all", 6)] {
            let filter = Filter {
                window: Some(window.into()),
                as_of: Some(as_of),
                ..Default::default()
            };
            let overview = store.dashboard(&filter).unwrap();
            assert_eq!(overview["summary"]["request_count"], count, "{window}");
            assert_eq!(overview["groups"][0]["request_count"], count, "{window}");
            assert_overview_matches_detail(
                &overview["groups"][0],
                &store.group("g", &filter).unwrap().unwrap()["group"],
            );
            assert_eq!(
                store.export(&filter, "jsonl").unwrap().lines().count(),
                count as usize
            );
        }
    }

    #[test]
    fn ledger_counts_once_and_import_identity_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
        let mut conn = store.writer_connection().unwrap();
        let mut first = record("1");
        first["source_event_id"] = json!("original");
        first["import_format"] = json!("observer-jsonl");
        let mut duplicate = first.clone();
        duplicate["id"] = json!("new-id");
        let second = record("2");
        assert_eq!(
            store
                .write_batch(&mut conn, &[first, duplicate, second])
                .unwrap(),
            2
        );
        let d = store.dashboard(&Filter::default()).unwrap();
        assert_eq!(d["summary"]["request_count"], 2);
        assert_eq!(d["summary"]["cost_usd"], 0.2);
        assert_eq!(d["groups"][0]["valid_count"], 2);
    }

    #[test]
    fn reopening_removes_legacy_index_without_changing_stored_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        let mut writer = store.writer_connection().unwrap();
        writer
            .execute_batch("CREATE INDEX answer_group ON answers(group_id,request_id)")
            .unwrap();
        store.write_batch(&mut writer, &[record("kept")]).unwrap();
        store.add_label("kept", "urgent", "correct").unwrap();
        let before = store.request("kept").unwrap().unwrap();
        drop(writer);
        let reopened = Store::open(&path, 7, 100).unwrap();
        assert_eq!(reopened.request("kept").unwrap().unwrap(), before);
        let connection = reopened.writer_connection().unwrap();
        let legacy_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name='answer_group'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy_count, 0);
        let overview = reopened.dashboard(&Filter::default()).unwrap();
        assert_eq!(overview["summary"]["request_count"], 1);
        assert_eq!(overview["groups"][0]["valid_count"], 1);
        assert_overview_matches_detail(
            &overview["groups"][0],
            &reopened.group("g", &Filter::default()).unwrap().unwrap()["group"],
        );
    }
    #[test]
    fn labels_roundtrip_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[record("1")])
            .unwrap();
        store.add_label("1", "urgent", "incorrect").unwrap();
        assert_eq!(
            store.request("1").unwrap().unwrap()["labels"][0]["label"],
            "incorrect"
        );
        assert!(
            store
                .export(&Filter::default(), "jsonl")
                .unwrap()
                .contains("incorrect")
        );
        store.delete_all().unwrap();
        assert!(store.request("1").unwrap().is_none());
        assert_eq!(
            store.dashboard(&Filter::default()).unwrap()["summary"]["request_count"],
            0
        );
    }
    #[test]
    fn unknown_cost_is_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
        let mut r = record("1");
        r["cost_usd"] = Value::Null;
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[r])
            .unwrap();
        let d = store.dashboard(&Filter::default()).unwrap();
        assert!(d["summary"]["cost_usd"].is_null());
        assert_eq!(d["summary"]["cost_known_requests"], 0);
    }

    #[test]
    fn unknown_model_stays_null_in_request_feed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("unknown.sqlite"), 7, 100).unwrap();
        let mut row = record("unknown-model");
        row["model"] = Value::Null;
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[row])
            .unwrap();
        let dashboard = store.dashboard(&Filter::default()).unwrap();
        assert!(dashboard["requests"][0]["model"].is_null());
        assert!(store.request("unknown-model").unwrap().unwrap()["model"].is_null());
    }

    #[test]
    fn restart_applies_age_and_count_retention_before_exposing_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retention.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        let mut old = record("old");
        old["timestamp"] = json!(Utc::now().timestamp_millis() - 2 * 86_400_000);
        old["answers"][0]["group_id"] = json!("old-group");
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                &[old, record("second"), record("latest")],
            )
            .unwrap();
        assert!(store.request("old").unwrap().is_some());
        let reopened = Store::open(&path, 1, 1).unwrap();
        assert!(reopened.request("old").unwrap().is_none());
        assert!(reopened.request("second").unwrap().is_none());
        assert!(reopened.request("latest").unwrap().is_some());
        let connection = reopened.writer_connection().unwrap();
        let orphans: i64 = connection
            .query_row(
                "SELECT count(*) FROM groups WHERE id='old-group'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn export_pages_share_an_explicit_time_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("export.sqlite"), 7, 1000).unwrap();
        let now = Utc::now().timestamp_millis();
        let records: Vec<_> = (0..205)
            .map(|index| {
                let mut row = record(&format!("record-{index}"));
                row["timestamp"] = json!(now - 3_630_000);
                row
            })
            .collect();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &records)
            .unwrap();
        // All rows were within the hour at export start, but the moving wall
        // clock would exclude them. No timing-dependent sleeps are needed.
        let filter = Filter {
            window: Some("1h".into()),
            as_of: Some(now - 60_000),
            ..Default::default()
        };
        let (first, cursor, through, more) = store.export_page(&filter, "jsonl", 0, None).unwrap();
        assert_eq!(first.lines().count(), 100);
        assert!(more);
        let (second, cursor, _, more) = store
            .export_page(&filter, "jsonl", cursor, Some(through))
            .unwrap();
        assert_eq!(second.lines().count(), 100);
        assert!(more);
        let (last, _, _, more) = store
            .export_page(&filter, "jsonl", cursor, Some(through))
            .unwrap();
        assert_eq!(last.lines().count(), 5);
        assert!(!more);
        let external: Filter =
            serde_json::from_value(json!({"window":"1h","as_of":now-60_000})).unwrap();
        assert!(external.as_of.is_none());
        assert!(
            store
                .export_page(&external, "jsonl", 0, None)
                .unwrap()
                .0
                .is_empty()
        );
    }

    type ParentReadHook = Box<dyn FnOnce(&str)>;

    thread_local! {
        static AFTER_PARENT_READ: std::cell::RefCell<Option<ParentReadHook>> = const {
            std::cell::RefCell::new(None)
        };
    }

    pub(super) fn after_parent_read(id: &str) {
        // Release the borrow before invoking the hook so it can make other reads.
        let hook = AFTER_PARENT_READ.with_borrow_mut(Option::take);
        if let Some(hook) = hook {
            hook(id);
        }
    }

    fn with_parent_read_hook<T>(hook: ParentReadHook, read: impl FnOnce() -> T) -> T {
        struct ResetHook;
        impl Drop for ResetHook {
            fn drop(&mut self) {
                AFTER_PARENT_READ.with_borrow_mut(|hook| *hook = None);
            }
        }

        // Each test thread owns its hook, and unwinding cannot leak it to a
        // subsequent test. Fail immediately if a test accidentally nests hooks.
        AFTER_PARENT_READ.with_borrow_mut(|current| {
            assert!(current.is_none(), "A parent-read hook is already installed");
            *current = Some(hook);
        });
        let _reset = ResetHook;
        let result = read();
        AFTER_PARENT_READ.with_borrow(|hook| {
            assert!(hook.is_none(), "The public read did not consume its hook");
        });
        result
    }

    #[derive(Clone, Copy, Debug)]
    enum SnapshotMutation {
        Delete,
        Relabel,
    }

    fn assert_parent_snapshot(mutation: SnapshotMutation, read: impl Fn(&Store) -> Value) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("read-snapshot.sqlite"), 7, 1000).unwrap();
        let mut writer = store.writer_connection().unwrap();
        store
            .write_batch(&mut writer, &[record("reviewed")])
            .unwrap();
        store.add_label("reviewed", "urgent", "correct").unwrap();
        let before = store.request("reviewed").unwrap().unwrap();
        assert_eq!(before["answers"].as_array().unwrap().len(), 1);
        assert_eq!(before["labels"][0]["label"], "correct");
        let expected = read(&store);
        let concurrent_store = store.clone();
        let observed = with_parent_read_hook(
            Box::new(move |id| {
                assert_eq!(id, "reviewed");
                // Commit on another connection after the public API reads the
                // parent, before it reads any answers or labels. Joining makes
                // the schedule deterministic without sleeps or polling.
                std::thread::spawn(move || {
                    let mut connection = concurrent_store.writer_connection().unwrap();
                    let tx = connection.transaction().unwrap();
                    let changed = match mutation {
                        SnapshotMutation::Delete => tx
                            .execute("DELETE FROM requests WHERE id='reviewed'", [])
                            .unwrap(),
                        SnapshotMutation::Relabel => tx
                            .execute(
                                "UPDATE labels SET label='incorrect' WHERE request_id='reviewed' AND key='urgent'",
                                [],
                            )
                            .unwrap(),
                    };
                    assert_eq!(changed, 1);
                    tx.commit().unwrap();
                })
                .join()
                .unwrap();
            }),
            || read(&store),
        );
        assert_eq!(observed, expected, "Mixed snapshots during {mutation:?}");

        let after = store.request("reviewed").unwrap();
        match mutation {
            SnapshotMutation::Delete => assert!(after.is_none()),
            SnapshotMutation::Relabel => {
                assert_eq!(after.unwrap()["labels"][0]["label"], "incorrect");
            }
        }
        let busy: i64 = writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 0, "A completed read must release its snapshot");
    }

    fn snapshot_export(store: &Store, format: &str) -> Value {
        let filter = Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let (page, cursor, through, more) = store.export_page(&filter, format, 0, None).unwrap();
        assert!(cursor > 0);
        assert_eq!(cursor, through);
        assert!(!more);
        if format == "jsonl" {
            serde_json::from_str(page.trim()).unwrap()
        } else {
            json!(page)
        }
    }

    #[test]
    fn request_keeps_parent_snapshot_during_concurrent_delete() {
        assert_parent_snapshot(SnapshotMutation::Delete, |store| {
            store.request("reviewed").unwrap().unwrap()
        });
    }

    #[test]
    fn request_keeps_parent_snapshot_during_concurrent_label_edit() {
        assert_parent_snapshot(SnapshotMutation::Relabel, |store| {
            store.request("reviewed").unwrap().unwrap()
        });
    }

    #[test]
    fn jsonl_export_keeps_parent_snapshot_during_concurrent_delete() {
        assert_parent_snapshot(SnapshotMutation::Delete, |store| {
            snapshot_export(store, "jsonl")
        });
    }

    #[test]
    fn jsonl_export_keeps_parent_snapshot_during_concurrent_label_edit() {
        assert_parent_snapshot(SnapshotMutation::Relabel, |store| {
            snapshot_export(store, "jsonl")
        });
    }

    #[test]
    fn csv_export_keeps_parent_snapshot_during_concurrent_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("csv-page-snapshot.sqlite"), 7, 100).unwrap();
        let mut writer = store.writer_connection().unwrap();
        store
            .write_batch(&mut writer, &[record("first"), record("second")])
            .unwrap();
        let expected = snapshot_export(&store, "csv");
        let concurrent = store.clone();
        let observed = with_parent_read_hook(
            Box::new(move |id| {
                assert_eq!(id, "first");
                // CSV needs only one SELECT per parent. Delete the next parent as
                // well to verify the whole page shares the selected IDs' snapshot.
                std::thread::spawn(move || {
                    assert_eq!(
                        concurrent
                            .writer_connection()
                            .unwrap()
                            .execute("DELETE FROM requests", [])
                            .unwrap(),
                        2
                    );
                })
                .join()
                .unwrap();
            }),
            || snapshot_export(&store, "csv"),
        );
        assert_eq!(observed, expected);
        assert!(store.is_empty().unwrap());
        let busy: i64 = writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 0, "A completed export page must release its snapshot");
    }

    #[test]
    fn csv_export_preserves_unknowns_escaping_and_all_answer_counts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("csv-summary.sqlite"), 7, 100).unwrap();
        let mut request = record("=ID(\"x\")");
        request["timestamp"] = Value::Null;
        request["imported_at"] = json!(Utc::now().timestamp_millis());
        request["source"] = json!("line,\n\"source\"");
        request["model"] = Value::Null;
        let mut invalid = request["answers"][0].clone();
        invalid["key"] = json!("invalid");
        invalid["valid"] = json!(false);
        invalid["value"] = Value::Null;
        request["answers"].as_array_mut().unwrap().push(invalid);
        request["labels"] = json!([{"key":"urgent","label":"correct"}]);
        let mut action = record("action");
        action["event_kind"] = json!("application_action");
        action["answers"] = json!([]);
        action["imported_at"] = request["imported_at"].clone();
        for key in [
            "timestamp",
            "model",
            "status",
            "duration_ms",
            "input_tokens",
            "output_tokens",
            "cost_usd",
            "cost_basis",
        ] {
            action[key] = Value::Null;
        }
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[request, action])
            .unwrap();
        let filter = Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        assert_eq!(
            store.export(&filter, "csv").unwrap(),
            concat!(
                "id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,event_kind\n",
                "\"'=ID(\"\"x\"\")\",\"\",\"line,\n\"\"source\"\"\",\"\",\"200\",\"4.0\",\"100\",\"3\",\"0.1\",\"synthetic\",\"2\",\"request\"\n",
                "\"action\",\"\",\"test\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"0\",\"application_action\"\n"
            )
        );
    }

    #[test]
    fn csv_cells_neutralize_disguised_formulas() {
        for value in [" =1+1", "\n=1+1", "\t=1+1", "＝1+1", " ＠SUM(1)", "\0=1+1"] {
            assert!(csv_cell(&json!(value)).starts_with("\"'"), "{value:?}");
        }
        assert_eq!(csv_cell(&json!("ordinary text")), "\"ordinary text\"");
    }

    #[test]
    fn group_observations_keep_unknown_event_time_and_break_ties_by_newest_request() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("group-observations.sqlite"), 7, 1000).unwrap();
        let now = Utc::now().timestamp_millis();
        let records: Vec<_> = (0..105)
            .map(|index| {
                let mut row = record(&format!("request-{index:03}"));
                row["timestamp"] = json!(now);
                row
            })
            .collect();
        let mut writer = store.writer_connection().unwrap();
        store.write_batch(&mut writer, &records).unwrap();
        let filter = Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let details = store.group("g", &filter).unwrap().unwrap();
        assert_eq!(details["requests"][0]["id"], "request-104");
        assert_eq!(details["answers"][0]["request_id"], "request-104");
        assert_eq!(details["answers"][99]["request_id"], "request-005");

        let mut imported = record("untimed");
        imported["timestamp"] = Value::Null;
        imported["imported_at"] = json!(now + 1000);
        store.write_batch(&mut writer, &[imported]).unwrap();
        let details = store.group("g", &filter).unwrap().unwrap();
        assert_eq!(details["answers"][0]["request_id"], "untimed");
        assert!(
            details["answers"][0]["timestamp"].is_null(),
            "Import time is not an original event timestamp"
        );
        assert_eq!(details["answers"][0]["imported_at"], now + 1000);
    }

    #[cfg(unix)]
    #[test]
    fn database_and_live_wal_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        let mut connection = store.writer_connection().unwrap();
        store
            .write_batch(&mut connection, &[record("private")])
            .unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let file = dir.path().join(format!("private.sqlite{suffix}"));
            let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{suffix}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn reopening_secures_existing_wal_files_through_a_database_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.sqlite");
        let store = Store::open(&path, 7, 100).unwrap();
        // Keep a connection alive so existing WAL/SHM files survive reopening.
        let mut connection = store.writer_connection().unwrap();
        store
            .write_batch(&mut connection, &[record("private-existing")])
            .unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let file = dir.path().join(format!("existing.sqlite{suffix}"));
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let alias = dir.path().join("database-link.sqlite");
        symlink(&path, &alias).unwrap();
        let reopened = Store::open(&alias, 7, 100).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let file = dir.path().join(format!("existing.sqlite{suffix}"));
            assert_eq!(
                std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600,
                "existing {suffix} remained readable by other users"
            );
        }
        assert!(reopened.request("private-existing").unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn opening_rejects_symlinked_database_peers_without_changing_the_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        for suffix in ["-wal", "-shm", "-journal"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("private.sqlite");
            Store::open(&path, 7, 100).unwrap();
            let unrelated = dir.path().join("unrelated.txt");
            std::fs::write(&unrelated, "unchanged").unwrap();
            std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o644)).unwrap();
            symlink(
                &unrelated,
                dir.path().join(format!("private.sqlite{suffix}")),
            )
            .unwrap();
            assert!(
                Store::open(&path, 7, 100).is_err(),
                "accepted {suffix} symlink"
            );
            assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "unchanged");
            assert_eq!(
                std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod dashboard_tests;
