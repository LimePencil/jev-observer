use super::*;

#[tokio::test]
async fn privacy_limited_captures_preserve_surrounding_batch_and_count_new_gaps_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("redaction-batch.sqlite"), 7, 100).unwrap();
    let collector = Collector::start(
        store.clone(),
        NormalizeOptions::default(),
        4 * 1024 * 1024,
        4,
        4,
    );
    let mut limited = capture("limited", false);
    limited.request =
        serde_json::to_vec(&json!({"questions":{}, "credentials":"s".repeat(2 * 1024 * 1024)}))
            .unwrap();
    let mut already_incomplete = capture("already-incomplete", false);
    already_incomplete.request = limited.request.clone();
    already_incomplete.capture_complete = false;
    for capture in [
        capture("before", false),
        limited,
        already_incomplete,
        capture("after", false),
    ] {
        collector.record_forwarded();
        collector.submit(capture, collector.reserve_capture().unwrap());
    }
    collector.shutdown().await;
    let health = collector.health();
    assert_eq!(health["persisted"], 4);
    assert_eq!(health["truncated"], 2);
    assert!(health["last_gap_at"].as_i64().unwrap() > 0);
    for field in ["dropped", "write_failures", "queue_depth", "queued_bytes"] {
        assert_eq!(health[field], 0, "{field}");
    }
    for id in ["before", "after"] {
        let record = store.request(id).unwrap().unwrap();
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["answers"][0]["valid"], true);
    }
    let connection = store.writer_connection().unwrap();
    let mut statement = connection
        .prepare("SELECT id FROM requests WHERE capture_complete=0 ORDER BY seq")
        .unwrap();
    let records = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| store.request(&row.unwrap()).unwrap().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["id"], records[1]["id"]);
    for record in records {
        assert_eq!(record["answers"], json!([]));
        assert!(
            record["normalization_error"]
                .as_str()
                .unwrap()
                .contains("redaction")
        );
    }
}

fn capture(id: &str, dense: bool) -> Capture {
    let questions = if dense {
        (0..2_000)
            .map(|index| (format!("q{index:04}"), Value::Null))
            .collect::<serde_json::Map<_, _>>()
    } else {
        serde_json::from_value(json!({"q":{"type":"noul","instructions":"Synthetic"}})).unwrap()
    };
    Capture {
        id: id.into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        source: if dense {
            "s".repeat(256)
        } else {
            "small".into()
        },
        task_version: Some(if dense { "v".repeat(256) } else { "v1".into() }),
        status: 200,
        request: serde_json::to_vec(&json!({"questions":questions})).unwrap(),
        response: if dense {
            br#"{"answers":{}}"#.to_vec()
        } else {
            br#"{"answers":{"q":{"type":"noul","noul":0.5}}}"#.to_vec()
        },
        capture_complete: true,
        ..Default::default()
    }
}

fn pending(capture: Capture, budget: &Arc<Semaphore>) -> Pending {
    Pending {
        size: (capture.request.len() + capture.response.len()) as u64,
        capture,
        _permit: budget.clone().try_acquire_owned().unwrap(),
        queued: Instant::now(),
    }
}

#[test]
fn normalized_work_bounds_json_escaping_and_saturates() {
    for value in [
        Value::Null,
        json!(false),
        json!(true),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(f64::MAX),
        json!(f64::MIN_POSITIVE),
        json!("\u{0000}\n\t\r\"\\日本語🧪"),
        json!({"\u{0001}\"": [null, true, 42, "🧪"], "empty": [{}, []]}),
    ] {
        let bytes = serde_json::to_vec(&value).unwrap().len();
        let full = normalized_charge(&value, usize::MAX);
        assert!(full >= bytes, "{value}");
        for limit in [0, 1, 4, 32, 100, usize::MAX] {
            assert_eq!(normalized_charge(&value, limit), full.min(limit));
        }
    }
    let large = json!(["x".repeat(MAX_BATCH_NORMALIZED_WORK), {"unused": [1, 2, 3]}]);
    assert_eq!(
        normalized_charge(&large, MAX_BATCH_NORMALIZED_WORK + 1),
        MAX_BATCH_NORMALIZED_WORK + 1
    );
}

#[test]
fn small_captures_keep_the_full_raw_batch_and_exact_content() {
    let budget = Arc::new(Semaphore::new(MAX_BATCH_RECORDS));
    let captures = (0..MAX_BATCH_RECORDS)
        .map(|index| capture(&index.to_string(), false))
        .collect::<Vec<_>>();
    let options = NormalizeOptions::default();
    let expected = captures
        .iter()
        .map(|item| normalize(item, &options))
        .collect::<Vec<_>>();
    let mut calls = Vec::new();
    {
        let mut batches = normalized_batches(
            captures
                .into_iter()
                .map(|item| pending(item, &budget))
                .collect(),
            |item| {
                calls.push(item.id.clone());
                Some(normalize(item, &options))
            },
        );
        let (pending, records) = batches.next().unwrap();
        assert_eq!(records, expected);
        assert_eq!(pending.len(), MAX_BATCH_RECORDS);
        assert_eq!(budget.available_permits(), 0);
        assert!(batches.next().is_none());
        drop(pending);
    }
    assert_eq!(
        calls,
        (0..MAX_BATCH_RECORDS)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(budget.available_permits(), MAX_BATCH_RECORDS);
}

#[test]
fn dense_mixed_batches_normalize_once_and_hold_deferred_permits() {
    let options = NormalizeOptions::default();
    let captures = [false, false, true, false, true, false]
        .into_iter()
        .enumerate()
        .map(|(index, dense)| capture(&index.to_string(), dense))
        .collect::<Vec<_>>();
    let expected = captures
        .iter()
        .map(|item| normalize(item, &options))
        .collect::<Vec<_>>();
    for record in [&expected[2], &expected[4]] {
        assert!(normalized_charge(record, usize::MAX) > MAX_BATCH_NORMALIZED_WORK);
        assert!(
            record["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["valid"] == false)
        );
    }
    assert!(captures.iter().all(|item| item.request.len() <= 262_144));
    let budget = Arc::new(Semaphore::new(captures.len()));
    let mut calls = Vec::new();
    let mut actual = Vec::new();
    {
        let mut batches = normalized_batches(
            captures
                .into_iter()
                .map(|item| pending(item, &budget))
                .collect(),
            |item| {
                calls.push(item.id.clone());
                Some(normalize(item, &options))
            },
        );
        let mut released = 0;
        for size in [2, 1, 1, 1, 1] {
            let (pending, records) = batches.next().unwrap();
            assert_eq!(records.len(), size);
            assert_eq!(budget.available_permits(), released);
            actual.extend(records);
            released += pending.len();
            drop(pending);
        }
        assert!(batches.next().is_none());
    }
    assert_eq!(actual, expected);
    assert_eq!(calls, ["0", "1", "2", "3", "4", "5"]);
    assert_eq!(budget.available_permits(), 6);
}

#[test]
fn exact_work_boundary_and_failed_normalization_keep_progress() {
    // Array punctuation + the null item make this exactly the work limit.
    let exact = json!(["x".repeat((MAX_BATCH_NORMALIZED_WORK - 10) / 6), null]);
    assert_eq!(
        normalized_charge(&exact, usize::MAX),
        MAX_BATCH_NORMALIZED_WORK
    );
    let budget = Arc::new(Semaphore::new(4));
    let inputs = (0..4)
        .map(|id| pending(capture(&id.to_string(), false), &budget))
        .collect();
    let mut calls = Vec::new();
    let sizes = normalized_batches(inputs, |item| {
        calls.push(item.id.clone());
        match item.id.as_str() {
            "0" | "3" => None,
            "1" => Some(exact.clone()),
            _ => Some(json!(true)),
        }
    })
    .map(|(pending, records)| (pending.len(), records.len()))
    .collect::<Vec<_>>();
    assert_eq!(sizes, [(2, 1), (2, 1)]);
    assert_eq!(calls, ["0", "1", "2", "3"]);
    assert_eq!(budget.available_permits(), 4);
}

#[test]
fn shutdown_drains_normalized_subbatches_and_recovers_after_one_failed_write() {
    for fail_first in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("normalized-batches.sqlite"), 7, 100).unwrap();
        if fail_first {
            store
                .writer_connection()
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER fail_first_capture BEFORE INSERT ON requests
                 WHEN NEW.id='first' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
                )
                .unwrap();
        }
        let options = NormalizeOptions::default();
        let captures = [
            capture("first", false),
            capture("second", false),
            capture("dense", true),
            capture("last", false),
        ];
        let expected = captures
            .iter()
            .map(|item| normalize(item, &options))
            .collect::<Vec<_>>();
        let budget = Arc::new(Semaphore::new(captures.len()));
        let counters = Arc::new(Counters::default());
        let (sender, receiver) = mpsc::sync_channel(captures.len());
        for capture in captures {
            let item = pending(capture, &budget);
            counters.captured.fetch_add(1, Ordering::Relaxed);
            counters.queue_depth.fetch_add(1, Ordering::Relaxed);
            counters
                .queued_bytes
                .fetch_add(item.size, Ordering::Relaxed);
            counters
                .pending_times
                .lock()
                .unwrap()
                .push_back(item.queued);
            sender.send(item).unwrap();
        }
        // Prequeue a complete raw batch, then request shutdown before starting
        // the real writer. This avoids timing-dependent grouping in the test.
        writer(
            store.clone(),
            options,
            receiver,
            counters.clone(),
            Arc::new(AtomicBool::new(true)),
        );
        drop(sender);
        let dropped = if fail_first { 2 } else { 0 };
        assert_eq!(counters.captured.load(Ordering::Relaxed), 4);
        assert_eq!(counters.persisted.load(Ordering::Relaxed), 4 - dropped);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), dropped);
        assert_eq!(
            counters.write_failures.load(Ordering::Relaxed),
            u64::from(fail_first)
        );
        assert_eq!(counters.queue_depth.load(Ordering::Relaxed), 0);
        assert_eq!(counters.queued_bytes.load(Ordering::Relaxed), 0);
        assert!(counters.pending_times.lock().unwrap().is_empty());
        assert_eq!(budget.available_permits(), 4);
        let connection = store.writer_connection().unwrap();
        let ids = connection
            .prepare("SELECT id FROM requests ORDER BY seq")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let expected = &expected[dropped as usize..];
        assert_eq!(
            ids,
            expected
                .iter()
                .map(|item| item["id"].as_str().unwrap())
                .collect::<Vec<_>>()
        );
        for record in expected {
            let mut saved = store
                .request(record["id"].as_str().unwrap())
                .unwrap()
                .unwrap();
            // Request details add this derived status flag to saved evidence.
            assert_eq!(
                saved.as_object_mut().unwrap().remove("failed"),
                Some(json!(false))
            );
            assert_eq!(&saved, record);
        }
    }
}
