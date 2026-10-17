use super::*;

fn record(id: &str, timestamp: i64, answer_count: usize) -> Value {
    let answers = (0..answer_count)
        .map(|index| {
            let key = format!("q{index}");
            let group = format!("{id}-{key}");
            json!({"key":key,"kind":"noul","group_id":group,
                "definition_id":group,"presentation_id":"presentation",
                "definition":{"type":"noul","instructions":group},
                "valid":true,"value":0.5,"warnings":[]})
        })
        .collect::<Vec<_>>();
    let labels = (0..answer_count)
        .map(|index| json!({"key":format!("q{index}"),"label":"correct","timestamp":timestamp,"source":"import"}))
        .collect::<Vec<_>>();
    json!({"schema_version":1,"id":id,"timestamp":timestamp,"source":"retention-test",
        "model":"fixture","status":200,"cost_basis":"unknown","capture_complete":true,
        "sample":false,"source_event_id":id,"import_format":"observer-jsonl",
        "answers":answers,"labels":labels})
}

fn counts(connection: &Connection) -> Vec<i64> {
    ["requests", "answers", "groups", "labels"]
        .map(|table| {
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        })
        .into()
}

fn make_due(connection: &Connection) {
    connection
        .execute("UPDATE maintenance SET value=0 WHERE key='pruned_at'", [])
        .unwrap();
}

fn last_pruned(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT value FROM maintenance WHERE key='pruned_at'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn assert_no_orphans(connection: &Connection) {
    let count: i64 = connection
        .query_row("SELECT count(*) FROM groups WHERE NOT EXISTS(SELECT 1 FROM answers WHERE answers.group_id=groups.id)", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
}

#[test]
fn no_parent_deletions_bound_group_work_and_complete_maintenance() {
    #[derive(Default)]
    struct Work {
        steps: i64,
    }
    struct Trace(*mut rusqlite::ffi::sqlite3);
    impl Drop for Trace {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0, 0, None, std::ptr::null_mut());
            }
        }
    }
    unsafe extern "C" fn measured(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        unsafe {
            let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
            let work = &mut *context.cast::<Work>();
            work.steps += i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                0,
            ));
        }
        0
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("unchanged.sqlite"), 7, 100).unwrap();
    let mut connection = store.writer_connection().unwrap();
    let now = Utc::now().timestamp_millis();
    let records = (0..16)
        .map(|index| record(&index.to_string(), now, 256))
        .collect::<Vec<_>>();
    store.write_batch(&mut connection, &records).unwrap();
    make_due(&connection);
    let mut work = Work::default();
    // The connection outlives the guard. Remove the callback before accessing
    // its stack context, including when maintenance returns an error.
    let handle = unsafe { connection.handle() };
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            handle,
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(measured),
            std::ptr::from_mut(&mut work).cast(),
        );
    }
    let trace = Trace(handle);
    let result = store.maintenance(&mut connection, false);
    drop(trace);
    assert!(result.unwrap());
    assert!(
        (1..2_000).contains(&work.steps),
        "No-op maintenance used {} VM steps",
        work.steps
    );
    assert_eq!(counts(&connection), [16, 4096, 4096, 4096]);
    let completed = last_pruned(&connection);
    assert!(completed >= now);
    assert!(!store.maintenance(&mut connection, false).unwrap());
    assert_eq!(last_pruned(&connection), completed);
    assert_no_orphans(&connection);
}

#[test]
fn age_and_count_retention_remove_exact_parent_metadata() {
    for age in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("churn.sqlite"),
            7,
            if age { 100 } else { 2 },
        )
        .unwrap();
        let mut connection = store.writer_connection().unwrap();
        let now = Utc::now().timestamp_millis();
        let first = record("first", if age { now - 8 * 86_400_000 } else { now }, 3);
        let second = record("second", now, 3);
        let third = record("third", now, 3);
        store
            .write_batch(&mut connection, &[first, second, third])
            .unwrap();
        assert_eq!(counts(&connection), [3, 9, 9, 9]);
        let kept = [
            store.request("second").unwrap(),
            store.request("third").unwrap(),
        ];
        make_due(&connection);
        assert!(store.maintenance(&mut connection, false).unwrap());
        assert_eq!(counts(&connection), [2, 6, 6, 6]);
        assert!(store.request("first").unwrap().is_none());
        assert_eq!(
            [
                store.request("second").unwrap(),
                store.request("third").unwrap()
            ],
            kept
        );
        assert_no_orphans(&connection);
    }
}

#[test]
fn forced_startup_repairs_existing_orphans_without_parent_deletions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("startup.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let mut connection = store.writer_connection().unwrap();
    store
        .write_batch(
            &mut connection,
            &[record("kept", Utc::now().timestamp_millis(), 2)],
        )
        .unwrap();
    let kept = store.request("kept").unwrap();
    // Emulate an existing supported history with unused metadata. Startup
    // still repairs it even though age/count policy removes no request.
    connection.execute("INSERT INTO groups SELECT 'orphan',key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter FROM groups LIMIT 1", []).unwrap();
    assert_eq!(counts(&connection), [1, 2, 3, 2]);
    drop(connection);
    drop(store);
    let reopened = Store::open(&path, 7, 100).unwrap();
    let connection = reopened.writer_connection().unwrap();
    assert_eq!(counts(&connection), [1, 2, 2, 2]);
    assert_eq!(reopened.request("kept").unwrap(), kept);
    assert_no_orphans(&connection);
}

#[test]
fn failed_group_cleanup_rolls_back_parent_children_and_bookkeeping() {
    for write_driven in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("rollback.sqlite"), 7, 1).unwrap();
        let mut connection = store.writer_connection().unwrap();
        let now = Utc::now().timestamp_millis();
        store
            .write_batch(&mut connection, &[record("old", now, 2)])
            .unwrap();
        if !write_driven {
            store
                .write_batch(&mut connection, &[record("new", now, 2)])
                .unwrap();
        }
        let before = counts(&connection);
        let old = store.request("old").unwrap();
        connection.execute_batch("CREATE TRIGGER prevent_group_cleanup BEFORE DELETE ON groups BEGIN SELECT RAISE(ABORT, 'synthetic cleanup failure'); END;").unwrap();
        make_due(&connection);
        let failed = if write_driven {
            store
                .write_batch(&mut connection, &[record("new", now, 2)])
                .map(|_| ())
        } else {
            store.maintenance(&mut connection, false).map(|_| ())
        };
        assert!(failed.is_err());
        assert_eq!(counts(&connection), before);
        assert_eq!(store.request("old").unwrap(), old);
        assert_eq!(store.request("new").unwrap().is_some(), !write_driven);
        assert_eq!(last_pruned(&connection), 0);
        assert_no_orphans(&connection);
        connection
            .execute_batch("DROP TRIGGER prevent_group_cleanup")
            .unwrap();
        if write_driven {
            store
                .write_batch(&mut connection, &[record("new", now, 2)])
                .unwrap();
        } else {
            assert!(store.maintenance(&mut connection, false).unwrap());
        }
        assert_eq!(counts(&connection), [1, 2, 2, 2]);
        assert!(store.request("old").unwrap().is_none());
        assert!(store.request("new").unwrap().is_some());
        assert!(last_pruned(&connection) > 0);
        assert_no_orphans(&connection);
    }
}
