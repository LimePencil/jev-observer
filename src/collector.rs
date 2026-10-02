use crate::{
    model::{Capture, NormalizeOptions, normalize},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(not(test))]
const IDLE_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
const IDLE_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(100);

struct Pending {
    capture: Capture,
    _permit: OwnedSemaphorePermit,
    size: u64,
    queued: Instant,
}

#[derive(Default)]
struct Counters {
    forwarded: AtomicU64,
    captured: AtomicU64,
    persisted: AtomicU64,
    dropped: AtomicU64,
    truncated: AtomicU64,
    write_failures: AtomicU64,
    maintenance_failures: AtomicU64,
    maintenance_failed: AtomicBool,
    last_maintenance_at: AtomicI64,
    queue_depth: AtomicU64,
    queued_bytes: AtomicU64,
    last_persisted_at: AtomicI64,
    last_gap_at: AtomicI64,
    lag_ms: AtomicU64,
    pending_times: Mutex<VecDeque<Instant>>,
    last_error: Mutex<Option<Value>>,
}

struct Inner {
    sender: SyncSender<Pending>,
    budget: Arc<Semaphore>,
    counters: Arc<Counters>,
    stopped: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    process_id: String,
    health_sequence: Mutex<u64>,
    capture_limit: usize,
    capture_slots: usize,
}

#[derive(Clone)]
pub struct Collector {
    inner: Arc<Inner>,
}

impl Collector {
    pub fn start(
        store: Store,
        options: NormalizeOptions,
        capture_limit: usize,
        capture_slots: usize,
        queue_capacity: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.max(1));
        let counters = Arc::new(Counters::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let writer_counters = counters.clone();
        let writer_stopped = stopped.clone();
        let worker = std::thread::Builder::new()
            .name("observer-writer".into())
            .spawn(move || writer(store, options, receiver, writer_counters, writer_stopped))
            .expect("start local writer thread");
        Self {
            inner: Arc::new(Inner {
                sender,
                budget: Arc::new(Semaphore::new(capture_slots)),
                counters,
                stopped,
                worker: Mutex::new(Some(worker)),
                process_id: uuid::Uuid::new_v4().to_string(),
                health_sequence: Mutex::new(0),
                capture_limit,
                capture_slots,
            }),
        }
    }

    pub fn reserve_capture(&self) -> Option<OwnedSemaphorePermit> {
        if self.inner.stopped.load(Ordering::Relaxed) {
            return None;
        }
        self.inner.budget.clone().try_acquire_owned().ok()
    }

    pub fn record_forwarded(&self) {
        self.inner
            .counters
            .forwarded
            .fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_skipped(&self) {
        gap(&self.inner.counters, 1);
    }

    pub fn submit(&self, capture: Capture, permit: OwnedSemaphorePermit) {
        let c = &self.inner.counters;
        if self.inner.stopped.load(Ordering::Acquire) {
            gap(c, 1);
            return;
        }
        if !capture.capture_complete {
            c.truncated.fetch_add(1, Ordering::Relaxed);
            c.last_gap_at
                .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
        }
        let size = (capture.request.len() + capture.response.len()) as u64;
        c.queue_depth.fetch_add(1, Ordering::Relaxed);
        c.queued_bytes.fetch_add(size, Ordering::Relaxed);
        let item = Pending {
            capture,
            _permit: permit,
            size,
            queued: Instant::now(),
        };
        // Preserve channel order for a live oldest-pending age even when the
        // database writer is blocked. This lock never surrounds database I/O.
        let mut pending = c.pending_times.lock().unwrap_or_else(|e| e.into_inner());
        pending.push_back(item.queued);
        match self.inner.sender.try_send(item) {
            Ok(()) => {
                c.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                pending.pop_back();
                c.queue_depth.fetch_sub(1, Ordering::Relaxed);
                c.queued_bytes.fetch_sub(size, Ordering::Relaxed);
                gap(c, 1);
            }
        }
    }

    pub fn health(&self) -> Value {
        // The dashboard and independent health endpoint can arrive out of
        // order. Serialize this short sample assembly so its sequence reflects
        // when counters were read, independent of network delivery times.
        let mut sequence = self
            .inner
            .health_sequence
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *sequence = sequence.saturating_add(1);
        let c = &self.inner.counters;
        let pending_age = c
            .pending_times
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .front()
            .map_or(0, |time| time.elapsed().as_millis() as u64);
        let sample = json!({
            "process_id":self.inner.process_id,
            "sample_sequence":*sequence,
            "forwarded":c.forwarded.load(Ordering::Relaxed),
            "captured":c.captured.load(Ordering::Relaxed),
            "persisted":c.persisted.load(Ordering::Relaxed),
            "dropped":c.dropped.load(Ordering::Relaxed),
            "truncated":c.truncated.load(Ordering::Relaxed),
            "write_failures":c.write_failures.load(Ordering::Relaxed),
            "maintenance_failures":c.maintenance_failures.load(Ordering::Relaxed),
            "maintenance_healthy":!c.maintenance_failed.load(Ordering::Relaxed),
            "last_maintenance_at":nullable_time(c.last_maintenance_at.load(Ordering::Relaxed)),
            "queue_depth":c.queue_depth.load(Ordering::Relaxed),
            "queued_bytes":c.queued_bytes.load(Ordering::Relaxed),
            "last_persisted_at":nullable_time(c.last_persisted_at.load(Ordering::Relaxed)),
            "last_gap_at":nullable_time(c.last_gap_at.load(Ordering::Relaxed)),
            "lag_ms":pending_age,
            "last_batch_lag_ms":c.lag_ms.load(Ordering::Relaxed),
            "active_captures":self.inner.capture_slots-self.inner.budget.available_permits(),
            "capture_limit":self.inner.capture_limit,
            "capture_slots":self.inner.capture_slots,
            "scope":"current process",
            "stopping":self.inner.stopped.load(Ordering::Relaxed),
            "last_error":c.last_error.lock().unwrap_or_else(|error| error.into_inner()).clone()
        });
        drop(sequence);
        sample
    }

    pub async fn shutdown(&self) {
        self.inner.stopped.store(true, Ordering::Release);
        let worker = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(worker) = worker {
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
        }
    }
}

fn nullable_time(value: i64) -> Option<i64> {
    if value == 0 { None } else { Some(value) }
}
fn gap(c: &Counters, count: u64) {
    c.dropped.fetch_add(count, Ordering::Relaxed);
    c.last_gap_at
        .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
}

fn error_category(error: &anyhow::Error) -> &'static str {
    for cause in error.chain() {
        if let Some(rusqlite::Error::SqliteFailure(code, _)) =
            cause.downcast_ref::<rusqlite::Error>()
        {
            use rusqlite::ErrorCode;
            return match code.code {
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => "database_busy",
                ErrorCode::DiskFull => "storage_full",
                ErrorCode::PermissionDenied | ErrorCode::ReadOnly => "permission_denied",
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => "database_unreadable",
                ErrorCode::CannotOpen | ErrorCode::SystemIoFailure => "storage_io",
                _ => "database_error",
            };
        }
        if let Some(error) = cause.downcast_ref::<std::io::Error>() {
            return match error.kind() {
                std::io::ErrorKind::PermissionDenied => "permission_denied",
                std::io::ErrorKind::StorageFull => "storage_full",
                _ => "storage_io",
            };
        }
    }
    "database_error"
}

fn diagnostic(c: &Counters, operation: &'static str, error: &anyhow::Error) {
    let category = error_category(error);
    let mut last = c
        .last_error
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // Never expose error strings: SQLite messages can contain SQL or values.
    if last
        .as_ref()
        .is_none_or(|last| last["operation"] != operation || last["category"] != category)
    {
        eprintln!("Observer storage failure: operation={operation}, category={category}");
    }
    *last = Some(
        json!({"operation":operation,"category":category,"at":chrono::Utc::now().timestamp_millis()}),
    );
}

fn open_writer(store: &Store, c: &Counters) -> Option<rusqlite::Connection> {
    match store.writer_connection() {
        Ok(connection) => Some(connection),
        Err(error) => {
            diagnostic(c, "open", &error);
            None
        }
    }
}

fn writer(
    store: Store,
    options: NormalizeOptions,
    receiver: Receiver<Pending>,
    c: Arc<Counters>,
    stopped: Arc<AtomicBool>,
) {
    let mut connection = open_writer(&store, &c);
    let mut last_maintenance = Instant::now();
    loop {
        let first = match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                if last_maintenance.elapsed() >= IDLE_MAINTENANCE_INTERVAL {
                    last_maintenance = Instant::now();
                    if connection.is_none() {
                        connection = open_writer(&store, &c);
                    }
                    let result =
                        connection
                            .as_mut()
                            .and_then(|conn| match store.maintenance(conn, false) {
                                Ok(ran) => Some(ran),
                                Err(error) => {
                                    diagnostic(&c, "retention", &error);
                                    None
                                }
                            });
                    if result.is_some() {
                        c.maintenance_failed.store(false, Ordering::Relaxed);
                        c.last_maintenance_at
                            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
                    } else {
                        c.maintenance_failures.fetch_add(1, Ordering::Relaxed);
                        c.maintenance_failed.store(true, Ordering::Relaxed);
                        connection = None;
                    }
                }
                continue;
            }
        };
        let mut batch = Vec::with_capacity(128);
        batch.push(first);
        let until = Instant::now() + Duration::from_millis(20);
        while batch.len() < 128 {
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match receiver.recv_timeout(remaining) {
                Ok(event) => batch.push(event),
                Err(_) => break,
            }
        }
        let mut records = Vec::with_capacity(batch.len());
        for item in &batch {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                normalize(&item.capture, &options)
            })) {
                Ok(record) => records.push(record),
                Err(_) => gap(&c, 1),
            }
        }
        if connection.is_none() {
            connection = open_writer(&store, &c);
        }
        let written =
            connection
                .as_mut()
                .and_then(|conn| match store.write_batch(conn, &records) {
                    Ok(written) => Some(written),
                    Err(error) => {
                        diagnostic(&c, "write", &error);
                        None
                    }
                });
        match written {
            Some(count) => {
                // write_batch checks retention in the same successful
                // transaction. Steady traffic may never enter the idle path.
                let checked_at = chrono::Utc::now().timestamp_millis();
                c.maintenance_failed.store(false, Ordering::Relaxed);
                c.last_maintenance_at.store(checked_at, Ordering::Relaxed);
                last_maintenance = Instant::now();
                c.persisted.fetch_add(count as u64, Ordering::Relaxed);
                c.last_persisted_at.store(checked_at, Ordering::Relaxed);
            }
            None => {
                c.write_failures.fetch_add(1, Ordering::Relaxed);
                gap(&c, records.len() as u64);
                connection = None;
            }
        }
        let lag = batch
            .iter()
            .map(|item| item.queued.elapsed().as_millis() as u64)
            .max()
            .unwrap_or(0);
        c.lag_ms.store(lag, Ordering::Relaxed);
        {
            let mut pending = c.pending_times.lock().unwrap_or_else(|e| e.into_inner());
            pending.drain(..batch.len());
        }
        for item in &batch {
            c.queue_depth.fetch_sub(1, Ordering::Relaxed);
            c.queued_bytes.fetch_sub(item.size, Ordering::Relaxed);
        }
        // Permits stay owned through persistence; the same memory budget covers the complete pipeline.
        drop(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_diagnostics_expose_categories_without_sql_or_sensitive_values() {
        let counters = Counters::default();
        let error = anyhow::Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            Some("SQL INSERT included provider-secret and request-state".into()),
        ))
        .context("private workspace filename");
        diagnostic(&counters, "write", &error);
        let saved = counters.last_error.lock().unwrap().clone().unwrap();
        assert_eq!(saved["category"], "storage_full");
        assert_eq!(saved["operation"], "write");
        assert!(saved["at"].as_i64().unwrap() > 0);
        for sensitive in ["provider-secret", "request-state", "filename", "INSERT"] {
            assert!(!saved.to_string().contains(sensitive));
        }
    }

    #[tokio::test]
    async fn health_samples_order_shared_clones_and_identify_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("health-samples.sqlite"), 7, 100).unwrap();
        let collector = Collector::start(store.clone(), NormalizeOptions::default(), 1024, 1, 1);
        let first = collector.health();
        let process_id = first["process_id"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(process_id).is_ok());
        assert_eq!(first["sample_sequence"], 1);
        collector.record_skipped();
        let second = collector.clone().health();
        assert_eq!(second["process_id"], process_id);
        assert_eq!(second["sample_sequence"], 2);
        assert_eq!(second["dropped"], 1);

        let mut samples = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    let collector = collector.clone();
                    scope.spawn(move || {
                        (0..8)
                            .map(|_| {
                                collector.record_skipped();
                                collector.health()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        samples.sort_by_key(|sample| sample["sample_sequence"].as_u64().unwrap());
        let mut dropped = 1;
        for (index, sample) in samples.iter().enumerate() {
            assert_eq!(sample["process_id"], process_id);
            assert_eq!(sample["sample_sequence"], (index + 3) as u64);
            let current = sample["dropped"].as_u64().unwrap();
            assert!(
                current >= dropped,
                "Later samples must not restore older counters"
            );
            dropped = current;
        }
        collector.shutdown().await;
        let restarted = Collector::start(store, NormalizeOptions::default(), 1024, 1, 1);
        let after_restart = restarted.health();
        assert_ne!(after_restart["process_id"], process_id);
        assert_eq!(after_restart["sample_sequence"], 1);
        assert_eq!(after_restart["dropped"], 0);
        restarted.shutdown().await;
    }

    #[tokio::test]
    async fn capture_pressure_never_waits_for_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("events.sqlite"), 7, 100).unwrap();
        let collector = Collector::start(store, NormalizeOptions::default(), 1024, 1, 1);
        let permit = collector.reserve_capture().unwrap();
        assert!(collector.reserve_capture().is_none());
        collector.record_skipped();
        assert_eq!(collector.health()["dropped"], 1);
        drop(permit);
        assert!(collector.reserve_capture().is_some());
        collector.shutdown().await;
    }

    #[tokio::test]
    async fn idle_retention_runs_without_provider_traffic_or_false_capture_counters() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("events.sqlite"), 7, 1000).unwrap();
        let mut record = crate::model::sample_records().remove(0);
        record["id"] = json!("expired-idle");
        record["timestamp"] = json!(chrono::Utc::now().timestamp_millis() - 8 * 86_400_000);
        let mut connection = store.writer_connection().unwrap();
        store.write_batch(&mut connection, &[record]).unwrap();
        assert!(store.request("expired-idle").unwrap().is_some());
        connection
            .execute("UPDATE maintenance SET value=0 WHERE key='pruned_at'", [])
            .unwrap();
        let collector = Collector::start(store.clone(), NormalizeOptions::default(), 1024, 1, 1);
        tokio::time::timeout(Duration::from_secs(2), async {
            while store.request("expired-idle").unwrap().is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("idle retention requires new provider traffic");
        let health = collector.health();
        for field in [
            "forwarded",
            "captured",
            "persisted",
            "dropped",
            "write_failures",
            "maintenance_failures",
        ] {
            assert_eq!(health[field], 0, "{field}");
        }
        assert_eq!(health["maintenance_healthy"], true);
        assert!(health["last_maintenance_at"].is_number());
        collector.shutdown().await;
    }

    #[tokio::test]
    async fn successful_capture_write_recovers_maintenance_health_without_idle_check() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("recovered.sqlite"), 7, 100).unwrap();
        let collector = Collector::start(store, NormalizeOptions::default(), 1024, 1, 1);
        collector
            .inner
            .counters
            .maintenance_failed
            .store(true, Ordering::Relaxed);
        collector
            .inner
            .counters
            .maintenance_failures
            .store(1, Ordering::Relaxed);
        collector
            .inner
            .counters
            .last_maintenance_at
            .store(1, Ordering::Relaxed);
        let capture = Capture {
            id: "recovered-write".into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            source: "test".into(),
            status: 200,
            request: br#"{"questions":{}}"#.to_vec(),
            response: br#"{"answers":{}}"#.to_vec(),
            capture_complete: true,
            ..Default::default()
        };
        let permit = collector.reserve_capture().unwrap();
        collector.record_forwarded();
        collector.submit(capture, permit);
        // Shutdown drains this accepted event and exits before any idle
        // maintenance check can hide a missing recovery in the write path.
        collector.shutdown().await;
        let health = collector.health();
        assert_eq!(health["persisted"], 1);
        assert_eq!(health["maintenance_healthy"], true);
        assert_eq!(health["maintenance_failures"], 1);
        assert_eq!(health["write_failures"], 0);
        assert_eq!(health["dropped"], 0);
        assert!(health["last_maintenance_at"].as_i64().unwrap() > 1);
        assert_eq!(health["last_maintenance_at"], health["last_persisted_at"]);
    }
}
