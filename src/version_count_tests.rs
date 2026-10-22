use super::*;
use sha2::{Digest, Sha256};

#[derive(Default, Debug)]
struct CountWork {
    statements: usize,
    steps: i64,
}

fn measured<T>(
    connection: &mut Connection,
    query: impl FnOnce(&mut Connection) -> T,
) -> (T, CountWork) {
    struct Trace(*mut rusqlite::ffi::sqlite3);
    impl Drop for Trace {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0, 0, None, std::ptr::null_mut());
            }
        }
    }
    unsafe extern "C" fn profile(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        unsafe {
            let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
            let sql = std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement)).to_bytes();
            if sql == Store::version_count_query(false).as_bytes()
                || sql == Store::version_count_query(true).as_bytes()
            {
                let work = &mut *context.cast::<CountWork>();
                work.statements += 1;
                work.steps += i64::from(rusqlite::ffi::sqlite3_stmt_status(
                    statement,
                    rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                    1,
                ));
            }
        }
        0
    }
    let mut work = CountWork::default();
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            connection.handle(),
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(profile),
            std::ptr::from_mut(&mut work).cast(),
        );
    }
    let trace = Trace(unsafe { connection.handle() });
    let result = query(connection);
    drop(trace);
    (result, work)
}

fn answer(group: &str, key: &str, family: Option<&str>) -> Value {
    json!({"key":key,"kind":"noul","group_id":group,"definition_id":group,
        "presentation_id":group,"definition":{"type":"noul","instructions":group},
        "family_id":family,"family_name":family,"valid":true,"value":0.75,"confidence":0.5})
}

fn record(id: &str, source: &str, answers: Vec<Value>) -> Value {
    // A fixed future timestamp survives age retention and makes the complete
    // response bytes deterministic across baseline and candidate test runs.
    json!({"schema_version":1,"id":id,"timestamp":4_000_000_000_000_i64,
        "source":source,"model":"count-fixture","status":200,"duration_ms":4.0,
        "input_tokens":5,"output_tokens":2,"cost_usd":0.25,"capture_complete":true,
        "sample":false,"answers":answers})
}

fn filter() -> Filter {
    Filter {
        window: Some("all".into()),
        as_of: Some(4_000_000_000_000),
        ..Default::default()
    }
}

fn evidence(label: &str, value: &Value, work: &CountWork) {
    println!(
        "VERSION_COUNT_EVIDENCE {label} {:x} {} {}",
        Sha256::digest(serde_json::to_vec(value).unwrap()),
        work.statements,
        work.steps
    );
}

#[test]
fn version_counts_reuse_actual_dashboard_and_detail_queries() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("counts.sqlite"), 7, 2000).unwrap();
    let records: Vec<_> = (0..128)
        .map(|index| {
            record(
                &format!("parent-{index:03}"),
                "source",
                (0..3)
                    .map(|key| answer(&format!("v-{index:03}-{key}"), &format!("q{key}"), None))
                    .collect(),
            )
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let (overview, overview_work) = measured(&mut store.reader().unwrap(), |connection| {
        Store::dashboard_from(connection, &filter()).unwrap()
    });
    let (detail, detail_work) = measured(&mut store.reader().unwrap(), |connection| {
        Store::group_from(&connection.transaction().unwrap(), "v-000-0", &filter())
            .unwrap()
            .unwrap()
    });
    evidence("strict-dashboard", &overview, &overview_work);
    evidence("strict-detail", &detail, &detail_work);
    let narrowed = Filter {
        search: Some("parent-000".into()),
        ..filter()
    };
    let narrowed_overview = store.dashboard(&narrowed).unwrap();
    assert_eq!(narrowed_overview["groups"].as_array().unwrap().len(), 3);
    for group in narrowed_overview["groups"].as_array().unwrap() {
        assert_eq!(group["request_count"], 1);
        assert_eq!(group["version_count"], 128);
    }
    let next_page = store
        .dashboard(&Filter {
            group_cursor: overview["group_next_cursor"].as_str().map(str::to_owned),
            ..filter()
        })
        .unwrap();
    assert_eq!(next_page["groups"].as_array().unwrap().len(), 100);
    for group in next_page["groups"].as_array().unwrap() {
        assert_eq!(group["version_count"], 128);
        assert!(
            !overview["groups"]
                .as_array()
                .unwrap()
                .iter()
                .any(|first| first["id"] == group["id"])
        );
    }
    assert_eq!(overview["groups"].as_array().unwrap().len(), 100);
    assert!(!overview["group_next_cursor"].is_null());
    assert_eq!(detail["versions"].as_array().unwrap().len(), 100);
    assert_eq!(detail["versions"][0]["id"], "v-000-0");
    for group in overview["groups"]
        .as_array()
        .unwrap()
        .iter()
        .chain(detail["versions"].as_array().unwrap())
    {
        assert_eq!(group["version_count"], 128);
    }
    assert_eq!(overview_work.statements, 3, "{overview_work:?}");
    assert_eq!(detail_work.statements, 1, "{detail_work:?}");
    assert!(overview_work.steps > 0 && overview_work.steps < 1500);
    assert!(detail_work.steps > 0 && detail_work.steps < 500);
}

#[test]
fn version_counts_separate_sources_keys_and_family_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("scopes.sqlite"), 7, 100).unwrap();
    let records = [
        record("a", "alpha", vec![answer("a-0", "same", None)]),
        record("b", "alpha", vec![answer("a-1", "same", None)]),
        record("c", "beta", vec![answer("b-0", "same", None)]),
        record("d", "alpha", vec![answer("f-0", "same", Some("same"))]),
        record("e", "alpha", vec![answer("f-1", "other", Some("same"))]),
        record("f", "alpha", vec![answer("f-2", "other", Some("same"))]),
        record("g", "alpha", vec![answer("f-3", "other", Some("same"))]),
        record("h", "alpha", vec![answer("f-4", "other", Some("same"))]),
    ];
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let (overview, overview_work) = measured(&mut store.reader().unwrap(), |connection| {
        Store::dashboard_from(connection, &filter()).unwrap()
    });
    let (detail, detail_work) = measured(&mut store.reader().unwrap(), |connection| {
        Store::group_from(&connection.transaction().unwrap(), "same", &filter())
            .unwrap()
            .unwrap()
    });
    evidence("mixed-dashboard", &overview, &overview_work);
    evidence("mixed-detail", &detail, &detail_work);
    let expected = BTreeMap::from([("a-0", 3), ("a-1", 3), ("b-0", 1), ("same", 5)]);
    let actual: BTreeMap<_, _> = overview["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            (
                group["id"].as_str().unwrap(),
                group["version_count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(detail["group"]["version_count"], 5);
    assert_eq!(detail["versions"].as_array().unwrap().len(), 5);
    for version in detail["versions"].as_array().unwrap() {
        assert_eq!(version["is_family"], false);
        assert_eq!(
            version["version_count"],
            if version["key"] == "same" { 3 } else { 4 }
        );
    }
    assert_eq!(overview_work.statements, 3, "{overview_work:?}");
    // One pooled family count plus the two member keys. The matching text
    // "same" must not reuse the family total for a strict member's key.
    assert_eq!(detail_work.statements, 3, "{detail_work:?}");

    let mut connection = store.reader().unwrap();
    let transaction = connection.transaction().unwrap();
    let mut counts = VersionCounts::new();
    for id in ["same", "f-0", "f-1", "a-0", "b-0", "f-2", "missing"] {
        assert_eq!(
            Store::group_metadata(&transaction, id, &mut counts).unwrap(),
            Store::group_metadata(&transaction, id, &mut VersionCounts::new()).unwrap(),
            "complete metadata must equal an independent uncached read for {id}"
        );
    }
}

#[test]
fn version_counts_follow_the_read_snapshot_and_refresh_after_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("snapshots.sqlite"), 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    store
        .write_batch(
            &mut writer,
            &[
                record("first", "source", vec![answer("g-0", "key", None)]),
                record("second", "source", vec![answer("g-1", "key", None)]),
            ],
        )
        .unwrap();
    let mut reader = store.reader().unwrap();
    let transaction = reader.transaction().unwrap();
    let mut counts = VersionCounts::new();
    let before = Store::group_metadata(&transaction, "g-0", &mut counts)
        .unwrap()
        .unwrap();
    assert_eq!(before["version_count"], 2);
    store
        .write_batch(
            &mut writer,
            &[record("third", "source", vec![answer("g-2", "key", None)])],
        )
        .unwrap();
    let same_snapshot = Store::group_metadata(&transaction, "g-1", &mut counts)
        .unwrap()
        .unwrap();
    assert_eq!(same_snapshot["version_count"], 2);
    assert_eq!(
        same_snapshot,
        Store::group_metadata(&transaction, "g-1", &mut VersionCounts::new())
            .unwrap()
            .unwrap()
    );
    drop(transaction);
    let after_import = store.group("g-0", &filter()).unwrap().unwrap();
    assert_eq!(after_import["group"]["version_count"], 3);
    assert_eq!(after_import["versions"].as_array().unwrap().len(), 3);
    // Simulate retention removing a parent and its now-unreferenced definition
    // atomically; a later endpoint must not keep the prior snapshot's total.
    writer.execute_batch("BEGIN; DELETE FROM requests WHERE id='third'; DELETE FROM groups WHERE id='g-2'; COMMIT;").unwrap();
    let after_delete = store.dashboard(&filter()).unwrap();
    for group in after_delete["groups"].as_array().unwrap() {
        assert_eq!(group["version_count"], 2);
    }
    assert_eq!(
        store.group("g-0", &filter()).unwrap().unwrap()["group"]["version_count"],
        2
    );
}
