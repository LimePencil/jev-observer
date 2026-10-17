use super::*;

#[derive(Default)]
struct QueryWork {
    total: i64,
    planning: i64,
    selected_parents: bool,
}

fn measured_detail(store: &Store, id: &str, filter: &Filter) -> (Option<Value>, QueryWork) {
    // Trace the actual detail path, including its planning queries. Unregister
    // before the stack context disappears even if the query returns an error.
    struct Trace<'a>(&'a Connection);
    impl Drop for Trace<'_> {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0.handle(), 0, None, std::ptr::null_mut());
            }
        }
    }
    unsafe extern "C" fn count_work(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        unsafe {
            let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
            let sql = std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement)).to_bytes();
            let work = &mut *context.cast::<QueryWork>();
            let steps = i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                0,
            ));
            work.total += steps;
            if sql.starts_with(b"SELECT count(*) FROM (SELECT 1 FROM requests")
                || sql.starts_with(b"SELECT EXISTS(SELECT 1 FROM groups")
            {
                work.planning += steps;
            }
            work.selected_parents |= sql.starts_with(b"CREATE TEMP TABLE group_request_ids");
        }
        0
    }
    let mut connection = store.reader().unwrap();
    let transaction = connection.transaction().unwrap();
    let mut work = QueryWork::default();
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            transaction.handle(),
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(count_work),
            std::ptr::from_mut(&mut work).cast(),
        );
    }
    let trace = Trace(&transaction);
    let detail = Store::group_from(&transaction, id, filter);
    drop(trace);
    (detail.unwrap(), work)
}

fn fixture(store: &Store, now: i64) {
    let records: Vec<_> = (0..4096)
        .map(|index| {
            let answer = |key: &str, group: &str, family: &str, value: f64| {
                json!({"key":key,"kind":"noul","group_id":group,
                    "family_id":family,"family_name":family,"definition_id":group,
                    "presentation_id":"presentation","definition":{"type":"noul"},
                    "valid":true,"value":value,"confidence":0.9,"warnings":[]})
            };
            let mut answers = vec![
                answer("wide", "wide-first", "wide-family", 0.5),
                answer("wide-second", "wide-second", "wide-family", 0.5),
            ];
            if index % 256 < 2 {
                answers.push(answer(
                    "focus",
                    if (index / 256) % 2 == 0 {
                        "focus-v0"
                    } else {
                        "focus-v1"
                    },
                    "focus-family",
                    0.25,
                ));
                answers.push(answer(
                    "focus-secondary",
                    "focus-secondary",
                    "focus-family",
                    0.75,
                ));
            }
            json!({"schema_version":1,"id":format!("request-{index:04}"),
                "timestamp":if index == 0 {None} else {Some(now-(index/8)*1000)},
                "imported_at":if index == 0 {Some(now)} else {None},
                "source":"fixture","model":if index % 2 == 0 {"primary"} else {"secondary"},
                "status":if index % 5 == 0 {500} else {200},"duration_ms":4.0,
                "input_tokens":100,"output_tokens":3,"cost_usd":0.25,
                "cost_basis":"synthetic","capture_complete":true,"sample":false,
                "answers":answers})
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
}

#[test]
fn sparse_group_details_bound_work_and_preserve_filtered_parent_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("group-plans.sqlite"), 7, 5000).unwrap();
    let now = Utc::now().timestamp_millis();
    fixture(&store, now);
    for (name, scope) in [
        ("all", json!({})),
        ("source", json!({"source":"fixture"})),
        ("model", json!({"model":"primary"})),
        (
            "combined",
            json!({"source":"fixture","model":"primary","search":"focus","status":"error"}),
        ),
        ("parent", json!({"group":"wide-family"})),
        ("intersection", json!({"group":"focus-v1"})),
        ("empty", json!({"group":"missing"})),
        (
            "cursor",
            json!({"request_cursor":"1:2","group_cursor":"1:ignored"}),
        ),
    ] {
        let mut filter: Filter = serde_json::from_value(scope).unwrap();
        filter.window = Some("all".into());
        filter.as_of = Some(now);
        for family in [false, true] {
            let id = if family { "focus-family" } else { "focus-v0" };
            let (detail, work) = measured_detail(&store, id, &filter);
            let detail = detail.unwrap();
            assert!(
                (1..250_000).contains(&work.total),
                "Detail scanned unrelated history: {} VM steps for {name}/{id}",
                work.total
            );
            assert!(work.selected_parents, "{name}/{id}");
            let mut expected: Vec<i64> = (0..4096)
                .filter(|index| index % 256 < 2)
                .filter(|index| family || (index / 256) % 2 == 0)
                .filter(|index| match name {
                    "model" => index % 2 == 0,
                    "combined" => index % 2 == 0 && index % 5 == 0,
                    "intersection" => (index / 256) % 2 == 1,
                    "empty" => false,
                    _ => true,
                })
                .collect();
            expected.sort_by_key(|index| (index / 8, std::cmp::Reverse(*index)));
            let count = expected.len();
            let answer_count = count * if family { 2 } else { 1 };
            assert_eq!(detail["group"]["request_count"], count);
            assert_eq!(detail["group"]["answer_count"], answer_count);
            assert_eq!(
                detail["group"]["input_tokens"],
                json!((count > 0).then_some(count * 100))
            );
            assert_eq!(detail["group"]["review_counts"]["unlabeled"], answer_count);
            assert_eq!(
                detail["group"]["error_count"],
                expected.iter().filter(|index| *index % 5 == 0).count()
            );
            assert_eq!(
                detail["requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|request| text(request, "id"))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|index| format!("request-{index:04}"))
                    .collect::<Vec<_>>()
            );
            let observations: Vec<_> = expected
                .iter()
                .flat_map(|index| {
                    let keys = if family {
                        vec!["focus", "focus-secondary"]
                    } else {
                        vec!["focus"]
                    };
                    keys.into_iter()
                        .map(move |key| (format!("request-{index:04}"), key.to_owned()))
                })
                .collect();
            assert_eq!(
                detail["answers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|answer| (text(answer, "request_id"), text(answer, "key")))
                    .collect::<Vec<_>>(),
                observations
            );
            let timed: i64 = detail["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .map(|bucket| bucket["requests"].as_i64().unwrap())
                .sum();
            assert_eq!(
                timed as usize,
                expected.iter().filter(|index| **index != 0).count()
            );
            if name == "all" {
                assert_eq!(detail, store.group(id, &filter).unwrap().unwrap());
            }
        }
    }

    // Temporary selection writes must remain usable on a read-only main
    // connection and must not advance the snapshot between detail queries.
    let mut reader = store.reader().unwrap();
    assert!(reader.is_readonly("main").unwrap());
    let transaction = reader.transaction().unwrap();
    let before: i64 = transaction
        .query_row("SELECT count(*) FROM requests", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, 4096);
    let mut later = store.request("request-0001").unwrap().unwrap();
    later["id"] = json!("later-parent");
    later["timestamp"] = json!(now + 1);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[later])
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let snapshot = Store::group_from(&transaction, "focus-family", &filter)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot["group"]["request_count"], 32);
    let selected: i64 = transaction
        .query_row("SELECT count(*) FROM temp.group_request_ids", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(selected, 32);
    drop(transaction);
    assert_eq!(
        store.group("focus-family", &filter).unwrap().unwrap()["group"]["request_count"],
        33
    );
}

#[test]
fn dense_and_narrow_group_scopes_keep_bounded_planning_and_existing_scans() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("group-fallbacks.sqlite"), 7, 5000).unwrap();
    let now = Utc::now().timestamp_millis();
    fixture(&store, now);
    let reader = store.reader().unwrap();
    assert!(Store::group_has_small_membership(&reader, "focus-family", 64).unwrap());
    assert!(!Store::group_has_small_membership(&reader, "focus-family", 63).unwrap());
    assert!(!Store::group_has_small_membership(&reader, "focus-family", 0).unwrap());
    for (from, expected) in [(now - 63_000, 128), (now - 62_000, 0)] {
        let filter = Filter {
            window: Some("all".into()),
            from: Some(from),
            to: Some(now),
            ..Default::default()
        };
        assert_eq!(
            Store::group_lookup_limit(&reader, &filter).unwrap(),
            expected
        );
    }
    for (id, scope, expected) in [
        ("wide-family", json!({}), 4096),
        ("wide-family", json!({"from":now-1000,"to":now}), 16),
        ("wide-family", json!({"source":"absent"}), 0),
        ("focus-family", json!({"from":now-1000,"to":now}), 2),
        ("focus-family", json!({"from":now+1,"to":now+1000}), 0),
    ] {
        let mut filter: Filter = serde_json::from_value(scope.clone()).unwrap();
        filter.window = Some("all".into());
        filter.as_of = Some(now);
        let (detail, work) = measured_detail(&store, id, &filter);
        assert!(!work.selected_parents, "{id}/{scope}");
        assert!(
            (1..100_000).contains(&work.planning),
            "{} planning steps for {id}/{scope}",
            work.planning
        );
        assert_eq!(detail.unwrap()["group"]["request_count"], expected);
    }
    let filter = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let (missing, work) = measured_detail(&store, "missing", &filter);
    assert!(missing.is_none());
    assert_eq!(work.planning, 0);
    assert!((1..1000).contains(&work.total));
}
