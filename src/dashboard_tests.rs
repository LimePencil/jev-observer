use super::*;

#[derive(Default)]
struct OverviewWork {
    overview: i64,
    selection: i64,
    answer_keys: i64,
    feed: i64,
    feed_statements: usize,
    catalogs: i64,
    timeline_bounds: i64,
}

fn measured_overview(store: &Store, filter: &Filter) -> (Result<Value>, OverviewWork) {
    struct Trace(*mut rusqlite::ffi::sqlite3);
    impl Drop for Trace {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0, 0, None, std::ptr::null_mut());
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
            let work = &mut *context.cast::<OverviewWork>();
            let steps = i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                1,
            ));
            if sql.starts_with(b"SELECT min(timestamp),max(timestamp) FROM requests")
                || sql.starts_with(b"SELECT r.timestamp FROM (SELECT seq,id,timestamp")
            {
                work.timeline_bounds += steps;
            }
            if sql.starts_with(b"SELECT id,timestamp,event_kind,answer_count") {
                work.overview += steps;
            }
            if sql.starts_with(b"WITH RECURSIVE keys(key)") {
                work.answer_keys += steps;
            }
            if sql.starts_with(b"INSERT INTO temp.dashboard_request_ids") {
                work.selection += steps;
            }
            if sql.starts_with(b"SELECT id,json_extract(data,'$.timestamp')") {
                work.feed += steps;
                work.feed_statements += 1;
            }
            if sql.starts_with(b"SELECT DISTINCT source FROM requests")
                || sql.starts_with(b"SELECT DISTINCT model FROM requests")
                || sql.starts_with(b"WITH RECURSIVE source_catalog")
                || sql.starts_with(b"WITH RECURSIVE model_catalog")
            {
                work.catalogs += steps;
            }
        }
        0
    }
    let mut connection = store.reader().unwrap();
    let mut work = OverviewWork::default();
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            connection.handle(),
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(count_work),
            std::ptr::from_mut(&mut work).cast(),
        );
    }
    // Unregister before inspecting the stack context, including error paths.
    let trace = Trace(unsafe { connection.handle() });
    let result = Store::dashboard_from(&mut connection, filter);
    drop(trace);
    (result, work)
}

#[test]
fn explicit_timeline_limits_skip_history_bounds_and_preserve_eligibility() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("explicit-bounds.sqlite"), 7, 5000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut records: Vec<_> = (0..4096)
        .map(|index| dashboard_record(&format!("bounds-{index}"), now - index / 2))
        .collect();
    let mut action = dashboard_record("bounds-action", now + 1);
    action["event_kind"] = json!("action");
    action["answers"] = json!([]);
    records.push(action);
    let mut unknown_time = dashboard_record("bounds-unknown", now);
    unknown_time["timestamp"] = Value::Null;
    unknown_time["imported_at"] = json!(now + 2);
    records.push(unknown_time);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let base = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    // Automatic bounds use bounded edge probes instead of a full history scan.
    for (from, to) in [
        (None, None),
        (Some(now - 3000), None),
        (None, Some(now + 3)),
    ] {
        let (snapshot, work) = measured_overview(
            &store,
            &Filter {
                from,
                to,
                ..base.clone()
            },
        );
        let snapshot = snapshot.unwrap();
        assert!(work.timeline_bounds > 0 && work.timeline_bounds < 40_000);
        assert_eq!(
            snapshot["timeline_meta"]["start"],
            from.unwrap_or(now - 2047)
        );
        assert_eq!(snapshot["timeline_meta"]["end"], to.unwrap_or(now) + 1);
    }
    // Public validation rejects inverted dates; the internal specification still
    // preserves its existing clamp for callers that already have a predicate.
    let inverted = Filter {
        from: Some(now + 20),
        to: Some(now + 10),
        ..base.clone()
    };
    assert!(store.dashboard(&inverted).is_err());
    let spec = Store::timeline_spec(&store.reader().unwrap(), "1=1", &[], &inverted).unwrap();
    assert_eq!((spec.start, spec.end), (now + 20, now + 20));
    for (from, to, expected) in [
        (now - 3000, now + 3, 4096),
        (now - 100, now - 96, 10),
        (now - 100, now - 100, 2),
        (now + 1, now + 2, 0), // action and unknown original time are not timeline evidence
        (now + 10, now + 20, 0),
        (i64::MIN, i64::MAX, 4096),
        (i64::MAX, i64::MAX, 0),
    ] {
        let filter = Filter {
            from: Some(from),
            to: Some(to),
            ..base.clone()
        };
        let (snapshot, work) = measured_overview(&store, &filter);
        let snapshot = snapshot.unwrap();
        assert!(
            work.timeline_bounds < 100,
            "Explicit bounds visited history: {} VM steps",
            work.timeline_bounds
        );
        assert_eq!(snapshot["timeline_meta"]["start"], from);
        assert_eq!(
            snapshot["timeline_meta"]["end"],
            to.max(from).saturating_add(1)
        );
        let timeline = snapshot["timeline"].as_array().unwrap();
        assert!(!timeline.is_empty() && timeline.len() <= 168);
        assert_eq!(
            timeline
                .iter()
                .map(|bin| bin["requests"].as_i64().unwrap())
                .sum::<i64>(),
            expected
        );
        let detail = store.group("dashboard-group", &filter).unwrap().unwrap();
        assert_eq!(detail["timeline"], snapshot["timeline"]);
        assert_eq!(detail["timeline_meta"], snapshot["timeline_meta"]);
    }
}

#[test]
fn automatic_timeline_bounds_match_aggregate_for_sparse_and_ineligible_edges() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("automatic-bounds.sqlite"), 7, 30_000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..20_000)
        .map(|index| {
            let mut record = dashboard_record(&format!("edge-{index}"), now - index);
            record["source"] = json!(if index == 10_000 { "rare" } else { "common" });
            if !(600..19_400).contains(&index) {
                record["timestamp"] = Value::Null;
                record["imported_at"] = json!(now - index);
            }
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let conn = store.writer_connection().unwrap();
    for source in [None, Some("rare"), Some("absent")] {
        for (from, to) in [
            (None, None),
            (Some(now - 15_000), None),
            (None, Some(now - 5000)),
        ] {
            let filter = Filter {
                window: Some("all".into()),
                source: source.map(str::to_owned),
                from,
                to,
                as_of: Some(now),
                ..Default::default()
            };
            let (predicate, args) = Store::predicate(&filter).unwrap();
            let (first, last): (Option<i64>, Option<i64>) = conn.query_row(
                &format!("SELECT min(timestamp),max(timestamp) FROM requests r WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL"),
                params_from_iter(&args), |row| Ok((row.get(0)?, row.get(1)?))
            ).unwrap();
            let expected_end = to.or(last).unwrap_or(now);
            let expected_start = from
                .or(first)
                .unwrap_or_else(|| expected_end.saturating_sub(86_400_000));
            let spec = Store::timeline_spec(&conn, &predicate, &args, &filter).unwrap();
            assert_eq!(
                (spec.start, spec.end),
                (expected_start, expected_end.max(expected_start))
            );
        }
    }
    // Make the edges eligible, then prove work is independent of the 20k rows.
    conn.execute(
        "UPDATE requests SET data=json_set(data,'$.timestamp',timestamp)",
        [],
    )
    .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let (snapshot, work) = measured_overview(&store, &filter);
    assert_eq!(snapshot.unwrap()["summary"]["request_count"], 20_000);
    assert!(
        work.timeline_bounds > 0 && work.timeline_bounds < 40_000,
        "{}",
        work.timeline_bounds
    );
}

#[test]
fn explicit_narrow_dates_bound_search_selection_work_and_preserve_results() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("dated-search.sqlite"), 7, 5000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..4096)
        .map(|index| dashboard_record(&format!("dated-{index}"), now - index / 2))
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    for (from, to, expected) in [
        (Some(now - 7), None, 16),
        (None, Some(now - 2040), 16),
        (Some(now - 100), Some(now - 96), 10),
        (Some(now - 100), Some(now - 100), 2),
        (Some(now + 1), None, 0),
        (None, Some(now - 2048), 0),
    ] {
        let filter = Filter {
            window: Some("all".into()),
            from,
            to,
            as_of: Some(now),
            ..Default::default()
        };
        let expected_snapshot = store.dashboard(&filter).unwrap();
        assert_snapshot_counts(&expected_snapshot, expected);
        let (actual, work) = measured_overview(
            &store,
            &Filter {
                search: Some("urgent".into()),
                ..filter
            },
        );
        assert_eq!(actual.unwrap(), expected_snapshot, "{from:?}..{to:?}");
        assert!(
            (1..3000).contains(&work.selection),
            "Narrow date search visited unrelated history: {} VM steps for {from:?}..{to:?}",
            work.selection
        );
    }

    // Wide explicit dates and an unbounded all-history search keep the same
    // complete response, including the next page across tied timestamps.
    for (from, to) in [(None, None), (Some(now - 4096), Some(now + 1))] {
        let filter = Filter {
            window: Some("all".into()),
            from,
            to,
            as_of: Some(now),
            ..Default::default()
        };
        let first = store.dashboard(&filter).unwrap();
        let cursor = first["feed_next_cursor"].as_str().unwrap();
        for request_cursor in [None, Some(cursor.to_owned())] {
            let filter = Filter {
                request_cursor,
                ..filter.clone()
            };
            assert_eq!(
                store
                    .dashboard(&Filter {
                        search: Some("urgent".into()),
                        ..filter.clone()
                    })
                    .unwrap(),
                store.dashboard(&filter).unwrap()
            );
        }
    }
}

#[test]
fn dashboard_catalogs_skip_long_duplicate_runs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("catalog-work.sqlite"), 7, 5000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..4096)
        .map(|index| {
            let mut record = dashboard_record(&format!("catalog-{index}"), now);
            record["answers"] = json!([]);
            record["source"] = json!(format!("source-{}", index % 3));
            record["model"] = json!(if index % 2 == 0 { "" } else { "model" });
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        status: Some("error".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let (snapshot, work) = measured_overview(&store, &filter);
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot["summary"]["request_count"], 0);
    assert_eq!(
        snapshot["sources"],
        json!(["source-0", "source-1", "source-2"])
    );
    assert_eq!(snapshot["models"], json!(["model"]));
    assert!(
        (1..1000).contains(&work.catalogs),
        "Catalogs visited duplicate entries: {} VM steps",
        work.catalogs
    );
}

#[test]
fn dashboard_catalogs_preserve_binary_order_empty_values_and_entry_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("catalog-values.sqlite"), 7, 3000).unwrap();
    let now = Utc::now().timestamp_millis();
    let filter = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let empty = store.dashboard(&filter).unwrap();
    assert_eq!(empty["sources"], json!([]));
    assert_eq!(empty["models"], json!([]));
    let mut values = ["", "\0", "A", "a", "a\0b", "a'quote", "é", "한글", "😀"]
        .map(str::to_owned)
        .to_vec();
    let catalog_record = |id: &str, value: &str| {
        let mut record = dashboard_record(id, now);
        record["answers"] = json!([]);
        record["source"] = json!(value);
        record["model"] = json!(value);
        record
    };
    let edges: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(index, value)| catalog_record(&format!("catalog-edge-{index}"), value))
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &edges)
        .unwrap();
    values.sort();
    let edge_snapshot = store.dashboard(&filter).unwrap();
    assert_eq!(edge_snapshot["sources"], json!(values));
    assert_eq!(
        edge_snapshot["models"],
        json!(
            values
                .iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        )
    );
    values.extend((0..1002).map(|index| format!("value-{index:04}")));
    let records: Vec<_> = values
        .iter()
        .cycle()
        .take(values.len() * 2)
        .enumerate()
        .map(|(index, value)| catalog_record(&format!("catalog-value-{index}"), value))
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    values.sort();
    let sources: Vec<_> = values.iter().take(1000).collect();
    let models: Vec<_> = values
        .iter()
        .filter(|value| !value.is_empty())
        .take(1000)
        .collect();
    for source in [None, Some("absent-source".into())] {
        let snapshot = store
            .dashboard(&Filter {
                source,
                ..filter.clone()
            })
            .unwrap();
        assert_eq!(snapshot["sources"], json!(sources));
        assert_eq!(snapshot["models"], json!(models));
    }
    store.delete_all().unwrap();
    let cleared = store.dashboard(&filter).unwrap();
    assert_eq!(cleared["sources"], json!([]));
    assert_eq!(cleared["models"], json!([]));
}

fn membership_fixture(store: &Store, now: i64) {
    let records: Vec<_> = (0..1024)
        .map(|index| {
            let mut record = dashboard_record(&format!("membership-{index}"), now - index / 8);
            record["source"] = json!("needle-source");
            if index % 16 == 0 {
                record["event_kind"] = json!("application_action");
                record["model"] = json!("actions");
                record["answers"] = json!([]);
            }
            if index == 0 {
                record["timestamp"] = Value::Null;
                record["imported_at"] = json!(now);
            }
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
}

#[test]
fn empty_dashboard_feed_avoids_history_work_and_validates_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty-feed.sqlite"), 7, 2000).unwrap();
    let now = Utc::now().timestamp_millis();
    membership_fixture(&store, now);
    let filter = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        status: Some("error".into()),
        ..Default::default()
    };
    let (result, work) = measured_overview(&store, &filter);
    let result = result.unwrap();
    assert_eq!(result, store.dashboard(&filter).unwrap());
    assert_eq!(result["summary"]["request_count"], 0);
    assert_eq!(result["summary"]["action_count"], 0);
    assert_eq!(result["requests"], json!([]));
    assert!(result["feed_next_cursor"].is_null());
    assert_eq!(work.feed_statements, 1);
    assert!(
        (1..100).contains(&work.feed),
        "Empty feed visited history: {} VM steps",
        work.feed
    );
    for cursor in ["invalid", "1:not-a-number"] {
        assert!(
            store
                .dashboard(&Filter {
                    request_cursor: Some(cursor.into()),
                    ..filter.clone()
                })
                .is_err()
        );
    }
    let valid_cursor = store
        .dashboard(&Filter {
            request_cursor: Some(format!("{now}:1")),
            ..filter
        })
        .unwrap();
    assert_eq!(valid_cursor["requests"], json!([]));
}

#[test]
fn full_search_membership_reuses_overview_work_and_preserves_filtered_pages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("full-membership.sqlite"), 7, 2000).unwrap();
    let now = Utc::now().timestamp_millis();
    membership_fixture(&store, now);
    let all = Filter {
        window: Some("all".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let (plain, plain_work) = measured_overview(&store, &all);
    let plain = plain.unwrap();
    let full = Filter {
        search: Some("needle".into()),
        ..all.clone()
    };
    let (selected, selected_work) = measured_overview(&store, &full);
    assert_eq!(selected.unwrap(), plain);
    assert!(plain_work.overview > 1024);
    assert!(
        selected_work.overview <= plain_work.overview + 100,
        "Full membership added per-parent work: {} versus {} VM steps",
        selected_work.overview,
        plain_work.overview
    );
    assert_eq!(plain["summary"]["request_count"], 960);
    assert_eq!(plain["summary"]["action_count"], 64);
    let next = plain["feed_next_cursor"].as_str().unwrap();
    for scope in [
        json!({"request_cursor":next}),
        json!({"source":"needle-source"}),
        json!({"model":"actions"}),
        json!({"from":now-8,"to":now}),
        json!({"group":"dashboard-group"}),
        json!({"status":"error"}),
    ] {
        let mut filter: Filter = serde_json::from_value(scope.clone()).unwrap();
        filter.window = Some("all".into());
        filter.as_of = Some(now);
        let expected = store.dashboard(&filter).unwrap();
        filter.search = Some("needle".into());
        assert_eq!(store.dashboard(&filter).unwrap(), expected, "{scope}");
        if scope["model"] == "actions" {
            assert_eq!(expected["summary"]["request_count"], 0);
            assert_eq!(expected["requests"].as_array().unwrap().len(), 64);
        }
    }
    let second = store
        .dashboard(&Filter {
            request_cursor: Some(next.into()),
            ..full
        })
        .unwrap();
    let first_ids: std::collections::HashSet<_> = plain["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(second["requests"].as_array().unwrap().len(), 100);
    assert!(
        second["requests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| !first_ids.contains(r["id"].as_str().unwrap()))
    );
}

#[test]
fn full_membership_counts_stay_in_the_read_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("snapshot-membership.sqlite"), 7, 10).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut original = dashboard_record("matching", now);
    original["source"] = json!("needle");
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[original])
        .unwrap();
    let mut reader = store.reader().unwrap();
    let transaction = reader.transaction().unwrap();
    let retained: i64 = transaction
        .query_row("SELECT count(*) FROM requests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, 1);
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[dashboard_record("later", now)],
        )
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        search: Some("needle".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let (predicate, args) = Store::dashboard_predicate(&transaction, &filter).unwrap();
    let (rows, cursor) = Store::request_page(&transaction, &predicate, &args, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "matching");
    assert!(cursor.is_none());
    drop(transaction);
    let selected = store.dashboard(&filter).unwrap();
    assert_eq!(selected["summary"]["request_count"], 1);
    assert_eq!(selected["requests"][0]["id"], "matching");
    assert_eq!(
        store
            .dashboard(&Filter {
                search: None,
                ..filter
            })
            .unwrap()["summary"]["request_count"],
        2
    );
}

#[test]
fn token_coverage_counts_requests_and_preserves_unknown_and_zero_usage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("token-coverage.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut records = Vec::new();
    for (id, input, output) in [
        ("input-only", Some(100), None),
        ("output-only", None, Some(20)),
        ("unknown", None, None),
        ("zero", Some(0), Some(0)),
    ] {
        let mut record = dashboard_record(id, now);
        record["source"] = json!(id);
        record["input_tokens"] = json!(input);
        record["output_tokens"] = json!(output);
        // Several answers still contribute only one parent request's usage.
        let mut second = record["answers"][0].clone();
        second["key"] = json!("second");
        record["answers"].as_array_mut().unwrap().push(second);
        records.push(record);
    }
    let mut action = dashboard_record("action", now);
    action["event_kind"] = json!("application_action");
    action["answers"] = json!([]);
    records.push(action);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    for (source, requests, input, output, input_known, output_known) in [
        (None, 4, Some(100), Some(20), 2, 2),
        (Some("input-only"), 1, Some(100), None, 1, 0),
        (Some("output-only"), 1, None, Some(20), 0, 1),
        (Some("unknown"), 1, None, None, 0, 0),
        (Some("zero"), 1, Some(0), Some(0), 1, 1),
        (Some("missing-source"), 0, None, None, 0, 0),
    ] {
        let dashboard = store
            .dashboard(&Filter {
                window: Some("all".into()),
                source: source.map(str::to_owned),
                ..Default::default()
            })
            .unwrap();
        let summary = &dashboard["summary"];
        assert_eq!(summary["request_count"], requests);
        assert_eq!(summary["input_tokens"], json!(input));
        assert_eq!(summary["output_tokens"], json!(output));
        assert_eq!(summary["input_tokens_known_requests"], input_known);
        assert_eq!(summary["output_tokens_known_requests"], output_known);
        assert_eq!(summary["input_tokens_overflow"], false);
        assert_eq!(summary["output_tokens_overflow"], false);
    }
}

#[test]
fn version_token_coverage_counts_parents_within_the_selected_scope() {
    for family in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("version-coverage.sqlite"), 7, 100).unwrap();
        let now = Utc::now().timestamp_millis();
        let records: Vec<_> = [
            ("input-only", Some(100), None),
            ("output-only", None, Some(20)),
            ("unknown", None, None),
            ("zero", Some(0), Some(0)),
            ("outside-time-range", Some(999), Some(999)),
        ]
        .into_iter()
        .map(|(id, input, output)| {
            let timestamp = if id == "outside-time-range" {
                now - 10_000
            } else {
                now
            };
            let mut record = dashboard_record(id, timestamp);
            record["model"] = json!(id);
            record["input_tokens"] = json!(input);
            record["output_tokens"] = json!(output);
            if family {
                record["answers"][0]["family_id"] = json!("family");
                record["answers"][0]["family_name"] = json!("Indexed questions");
            }
            let mut second = record["answers"][0].clone();
            second["key"] = json!("second");
            second["group_id"] = json!("second-group");
            record["answers"].as_array_mut().unwrap().push(second);
            record
        })
        .collect();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &records)
            .unwrap();

        for (model, requests, input, output, input_known, output_known) in [
            (None, 4, Some(100), Some(20), 2, 2),
            (Some("input-only"), 1, Some(100), None, 1, 0),
            (Some("output-only"), 1, None, Some(20), 0, 1),
            (Some("unknown"), 1, None, None, 0, 0),
            (Some("zero"), 1, Some(0), Some(0), 1, 1),
            (Some("missing-model"), 0, None, None, 0, 0),
        ] {
            let detail = store
                .group(
                    if family { "family" } else { "dashboard-group" },
                    &Filter {
                        window: Some("all".into()),
                        from: Some(now - 1_000),
                        to: Some(now),
                        model: model.map(str::to_owned),
                        ..Default::default()
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(
                detail["group"]["answer_count"],
                requests * if family { 2 } else { 1 }
            );
            for group in
                std::iter::once(&detail["group"]).chain(detail["versions"].as_array().unwrap())
            {
                assert_eq!(group["request_count"], requests);
                assert_eq!(group["input_tokens"], json!(input));
                assert_eq!(group["output_tokens"], json!(output));
                assert_eq!(group["input_tokens_known_requests"], input_known);
                assert_eq!(group["output_tokens_known_requests"], output_known);
                assert_eq!(group["input_tokens_overflow"], false);
                assert_eq!(group["output_tokens_overflow"], false);
            }
        }
    }
}

#[test]
fn group_parent_metrics_do_not_scan_unrelated_source_or_model_history() {
    // Count actual parent-aggregate VM steps independently of host speed.
    // Other group queries keep their own plans. Reset the trace before its
    // stack context goes away, including when a query or assertion fails.
    struct ParentWorkTrace<'a>(&'a Connection);
    impl Drop for ParentWorkTrace<'_> {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0.handle(), 0, None, std::ptr::null_mut());
            }
        }
    }
    unsafe extern "C" fn count_parent_work(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
        let sql = unsafe { std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement)) };
        if sql.to_bytes().starts_with(b"SELECT avg(duration_ms)")
            || sql
                .to_bytes()
                .starts_with(b"SELECT duration_ms,cost_usd,input_tokens")
        {
            unsafe {
                *context.cast::<i64>() += i64::from(rusqlite::ffi::sqlite3_stmt_status(
                    statement,
                    rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                    0,
                ));
            }
        }
        0
    }

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("group-parent-metrics.sqlite"), 7, 3000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut first = dashboard_record("first", now);
    first["answers"][0]["family_id"] = json!("metrics-family");
    first["answers"][0]["family_name"] = json!("Metrics family");
    let mut companion = first["answers"][0].clone();
    companion["key"] = json!("companion");
    companion["group_id"] = json!("companion-group");
    first["answers"].as_array_mut().unwrap().push(companion);
    let mut second = first.clone();
    second["id"] = json!("second");
    second["timestamp"] = json!(now + 1);
    second["input_tokens"] = Value::Null;
    second["output_tokens"] = json!(20);
    second["duration_ms"] = json!(8.0);
    second["cost_usd"] = json!(0.5);
    second["status"] = json!(500);
    let mut records = vec![first, second];
    records.extend((0..2048).map(|index| {
        let mut unrelated = dashboard_record(&format!("unrelated-{index:04}"), now);
        unrelated["answers"] = json!([]);
        unrelated
    }));
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let connection = store.reader().unwrap();
    for (scope, first_matches, second_matches) in [
        (json!({}), true, true),
        (json!({"source":"dashboard-test"}), true, true),
        (json!({"model":"test-model"}), true, true),
        (
            json!({"source":"dashboard-test","model":"test-model","search":"first"}),
            true,
            false,
        ),
        (
            json!({"source":"dashboard-test","status":"error"}),
            false,
            true,
        ),
        (
            json!({"source":"dashboard-test","group":"companion-group"}),
            true,
            true,
        ),
        (
            json!({"source":"dashboard-test","group":"missing"}),
            false,
            false,
        ),
        (json!({"source":"missing"}), false, false),
        (
            json!({"source":"dashboard-test","from":now+1,"to":now+1}),
            false,
            true,
        ),
    ] {
        let first = i64::from(first_matches);
        let second = i64::from(second_matches);
        let count = first + second;
        let input = first_matches.then_some(100);
        let output = (count > 0).then_some(first * 3 + second * 20);
        let latency = (count > 0).then(|| (first * 4 + second * 8) as f64 / count as f64);
        let cost = (count > 0).then_some(first as f64 * 0.25 + second as f64 * 0.5);
        let mut filter: Filter = serde_json::from_value(scope.clone()).unwrap();
        filter.window = Some("all".into());
        for family in [false, true] {
            let mut parent_steps = 0_i64;
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(
                    connection.handle(),
                    rusqlite::ffi::SQLITE_TRACE_PROFILE,
                    Some(count_parent_work),
                    std::ptr::from_mut(&mut parent_steps).cast(),
                );
            }
            let trace = ParentWorkTrace(&connection);
            let metrics = Store::group_metrics(
                &connection,
                if family {
                    "metrics-family"
                } else {
                    "dashboard-group"
                },
                family,
                &filter,
                false,
            );
            drop(trace);
            let metrics = metrics.unwrap();
            assert!(
                (1..4000).contains(&parent_steps),
                "Parent aggregate visited unrelated history: {parent_steps} VM steps for {scope} (family={family})"
            );
            assert_eq!(metrics["input_tokens"], json!(input));
            assert_eq!(metrics["output_tokens"], json!(output));
            assert_eq!(metrics["input_tokens_known_requests"], first);
            assert_eq!(metrics["output_tokens_known_requests"], count);
            assert_eq!(metrics["error_count"], second);
            assert_eq!(metrics["mean_latency_ms"], json!(latency));
            assert_eq!(metrics["cost_usd"], json!(cost));
            assert_eq!(metrics["cost_known_requests"], count);
            assert_eq!(
                metrics["review_counts"]["unlabeled"],
                count * if family { 2 } else { 1 }
            );
        }
    }
}

#[test]
fn sample_indicator_includes_actions_without_changing_inference_accounting() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("sample-actions.sqlite"), 7, 100).unwrap();
    let options = crate::model::NormalizeOptions::default();
    let receipt = include_str!("../fixtures/model/jevrouter-receipt.json");
    let synthetic = crate::model::import_records(receipt, "jevrouter-receipt", &options).unwrap();
    assert_eq!(synthetic[0]["sample"], true);
    assert_eq!(synthetic[0]["event_kind"], "application_action");
    store
        .write_batch(&mut store.writer_connection().unwrap(), &synthetic)
        .unwrap();
    let all = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let action_only = store.dashboard(&all).unwrap();
    assert_eq!(action_only["sample"], true);
    assert_eq!(action_only["summary"]["request_count"], 0);
    assert_eq!(action_only["summary"]["action_count"], 1);
    assert_eq!(action_only["summary"]["sample_count"], 0);
    assert!(action_only["summary"]["input_tokens"].is_null());
    assert!(action_only["summary"]["cost_usd"].is_null());

    let mut ordinary_receipt: Value = serde_json::from_str(receipt).unwrap();
    ordinary_receipt["decision_id"] = json!("ordinary-decision");
    ordinary_receipt["provenance"]["jev_provider"] = json!("typesafe");
    ordinary_receipt["raw_jev"]["model"] = json!("ordinary-action");
    let mut more =
        crate::model::import_records(&ordinary_receipt.to_string(), "jevrouter-receipt", &options)
            .unwrap();
    assert_eq!(more[0]["sample"], false);
    let mut live = dashboard_record("live-request", Utc::now().timestamp_millis());
    live["model"] = json!("ordinary-request");
    more.push(live);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &more)
        .unwrap();
    let mixed = store.dashboard(&all).unwrap();
    assert_eq!(mixed["sample"], true);
    assert_eq!(mixed["summary"]["request_count"], 1);
    assert_eq!(mixed["summary"]["action_count"], 2);
    assert_eq!(mixed["summary"]["sample_count"], 0);
    assert_eq!(mixed["summary"]["input_tokens"], 100);
    assert_eq!(mixed["summary"]["cost_usd"], 0.25);

    for (model, sample, requests, actions) in [
        ("jevrouter-demo", true, 0, 1),
        ("ordinary-action", false, 0, 1),
        ("ordinary-request", false, 1, 0),
        ("absent", false, 0, 0),
    ] {
        let filtered = store
            .dashboard(&Filter {
                model: Some(model.into()),
                ..all.clone()
            })
            .unwrap();
        assert_eq!(filtered["sample"], sample, "{model}");
        assert_eq!(filtered["summary"]["request_count"], requests);
        assert_eq!(filtered["summary"]["action_count"], actions);
        assert_eq!(filtered["summary"]["sample_count"], 0);
    }
}

#[test]
fn overflowing_token_totals_preserve_other_metrics_coverage_and_filtered_exactness() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("token-overflow.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = [
        (Some(i64::MAX), Some(0), Some(4.0), Some(0.25)),
        (Some(1), Some(i64::MAX), None, None),
        (Some(0), Some(1), Some(8.0), Some(0.5)),
        (None, None, Some(0.0), Some(0.0)),
        (Some(0), Some(0), Some(2.0), Some(0.25)),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (input, output, duration, cost))| {
        let mut record = dashboard_record(&format!("overflow-{index}"), now + index as i64);
        record["input_tokens"] = json!(input);
        record["output_tokens"] = json!(output);
        record["duration_ms"] = json!(duration);
        record["cost_usd"] = json!(cost);
        // Unknown status and explicit failure exercise SQL's nullable predicate.
        record["status"] = json!([Some(200), None, Some(500), Some(200), Some(200)][index]);
        record["answers"][0]["family_id"] = json!("overflow-family");
        record["answers"][0]["family_name"] = json!("Overflow family");
        let mut second = record["answers"][0].clone();
        second["key"] = json!("second");
        second["group_id"] = json!("second-group");
        record["answers"].as_array_mut().unwrap().push(second);
        record
    })
    .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    // Directional overflow, both overflowed, exact boundary, zeros, unknowns,
    // and empty scopes remain distinct. Later zero/unknown rows cannot reset it.
    for (start, end, input, output, input_overflow, output_overflow, known, count) in [
        (0, 0, Some(i64::MAX), Some(0), false, false, 1, 1),
        (0, 1, None, Some(i64::MAX), true, false, 2, 2),
        (1, 2, Some(1), None, false, true, 2, 2),
        (0, 4, None, None, true, true, 4, 5),
        (3, 3, None, None, false, false, 0, 1),
        (4, 4, Some(0), Some(0), false, false, 1, 1),
        (6, 6, None, None, false, false, 0, 0),
    ] {
        let filter = Filter {
            window: Some("all".into()),
            from: Some(now + start),
            to: Some(now + end),
            ..Default::default()
        };
        let dashboard = store.dashboard(&filter).unwrap();
        let check_usage = |metrics: &Value| {
            assert_eq!(metrics["request_count"], count);
            assert_eq!(metrics["input_tokens"], json!(input));
            assert_eq!(metrics["output_tokens"], json!(output));
            assert_eq!(metrics["input_tokens_overflow"], input_overflow);
            assert_eq!(metrics["output_tokens_overflow"], output_overflow);
            assert_eq!(metrics["input_tokens_known_requests"], known);
            assert_eq!(metrics["output_tokens_known_requests"], known);
        };
        check_usage(&dashboard["summary"]);
        assert_eq!(dashboard["summary"]["answer_count"], count * 2);
        assert_eq!(
            dashboard["requests"].as_array().unwrap().len(),
            count as usize
        );
        let connection = store.reader().unwrap();
        let (predicate, args) = Store::predicate(&filter).unwrap();
        // Independent SQL reference checks the unaffected parent metrics.
        let (latency, cost, cost_known, errors): (Option<f64>, Option<f64>, i64, i64) = connection.query_row(
            &format!("SELECT avg(duration_ms),sum(cost_usd),count(cost_usd),coalesce(sum({FAILED}),0) FROM requests r WHERE {predicate}"),
            params_from_iter(&args), |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).unwrap();
        assert_eq!(dashboard["summary"]["cost_usd"], json!(cost));
        assert_eq!(dashboard["summary"]["cost_known_requests"], cost_known);
        assert_eq!(dashboard["summary"]["error_count"], errors);
        for id in ["dashboard-group", "overflow-family"] {
            let detail = store.group(id, &filter).unwrap().unwrap();
            for group in
                std::iter::once(&detail["group"]).chain(detail["versions"].as_array().unwrap())
            {
                check_usage(group);
                assert_eq!(group["mean_latency_ms"], json!(latency));
                assert_eq!(group["cost_usd"], json!(cost));
                assert_eq!(group["cost_known_requests"], cost_known);
                assert_eq!(group["error_count"], errors);
            }
        }
    }
    assert_eq!(
        store.request("overflow-0").unwrap().unwrap()["input_tokens"],
        i64::MAX
    );
    let exported = store
        .export(
            &Filter {
                window: Some("all".into()),
                ..Default::default()
            },
            "jsonl",
        )
        .unwrap();
    let exported: Vec<Value> = exported
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(exported[0]["input_tokens"], i64::MAX);
}

#[test]
fn group_metrics_preserve_unrelated_database_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("metrics-error.sqlite"), 7, 100).unwrap();
    let connection = store.writer_connection().unwrap();
    connection.execute("DROP TABLE requests", []).unwrap();
    let error = Store::group_metrics(
        &connection,
        "missing-group",
        false,
        &Filter::default(),
        false,
    )
    .unwrap_err();
    let Some(rusqlite::Error::SqliteFailure(code, Some(message))) =
        error.downcast_ref::<rusqlite::Error>()
    else {
        panic!("expected original SQLite failure: {error}")
    };
    assert_eq!(code.extended_code, rusqlite::ffi::SQLITE_ERROR);
    assert_eq!(message, "no such table: requests");
}

#[test]
fn individually_valid_imported_token_counts_cannot_break_aggregates() {
    let options = crate::model::NormalizeOptions::default();
    let records: Vec<_> = ["large-a", "large-b"].into_iter().map(|id| {
        crate::model::normalize(&crate::model::Capture {
            id: id.into(), source: "large-import".into(),
            timestamp: Utc::now().timestamp_millis(), status: 200,
            request: br#"{"questions":{"q":{"type":"noul","instructions":"Proceed?"}}}"#.to_vec(),
            response: serde_json::to_vec(&json!({"answers":{"q":{"type":"noul","noul":0.7}},"usage":{"input_tokens":i64::MAX,"output_tokens":1}})).unwrap(),
            capture_complete: true, ..Default::default()
        }, &options).to_string()
    }).collect();
    let imported =
        crate::model::import_records(&records.join("\n"), "observer-jsonl", &options).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("valid-large-import.sqlite"), 7, 100).unwrap();
    assert_eq!(
        store
            .write_batch(&mut store.writer_connection().unwrap(), &imported)
            .unwrap(),
        2
    );
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let dashboard = store.dashboard(&filter).unwrap();
    assert_eq!(dashboard["summary"]["input_tokens"], Value::Null);
    assert_eq!(dashboard["summary"]["input_tokens_overflow"], true);
    assert_eq!(dashboard["summary"]["input_tokens_known_requests"], 2);
    assert_eq!(dashboard["summary"]["output_tokens"], 2);
    assert_eq!(dashboard["summary"]["output_tokens_overflow"], false);
    let detail = store
        .group(
            imported[0]["answers"][0]["group_id"].as_str().unwrap(),
            &filter,
        )
        .unwrap()
        .unwrap();
    assert_eq!(detail["group"]["input_tokens"], Value::Null);
    assert_eq!(detail["group"]["input_tokens_overflow"], true);
    assert_eq!(detail["group"]["input_tokens_known_requests"], 2);
    assert_eq!(detail["group"]["output_tokens"], 2);
    assert_eq!(detail["group"]["output_tokens_overflow"], false);
}

#[test]
fn empty_and_extreme_timeline_ranges_are_bounded_without_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty-range.sqlite"), 7, 100).unwrap();
    for (from, to) in [(0, 0), (0, 1), (i64::MIN, i64::MAX), (i64::MAX, i64::MAX)] {
        let filter = Filter {
            window: Some("all".into()),
            from: Some(from),
            to: Some(to),
            ..Default::default()
        };
        let result = store.dashboard(&filter).unwrap();
        let bins = result["timeline"].as_array().unwrap();
        assert!(!bins.is_empty() && bins.len() <= 168);
        assert!(bins.iter().all(|bin| bin["requests"] == 0));
        assert_eq!(result["timeline_meta"]["start"], from);
        assert_eq!(result["timeline_meta"]["end"], to.saturating_add(1));
    }
}

#[test]
fn transfer_failures_dates_and_empty_timeline_bins_preserve_the_complete_range() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("outcomes.sqlite"), 30, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut failed = dashboard_record("body-aborted", now - 1_000);
    failed["transport_error"] = json!("Upstream response transfer failed");
    failed["capture_complete"] = json!(false);
    let healthy = dashboard_record("healthy", now - 200 * 3_600_000);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[healthy, failed])
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        from: Some(now - 240 * 3_600_000),
        to: Some(now),
        ..Default::default()
    };
    let dashboard = store.dashboard(&filter).unwrap();
    assert_eq!(dashboard["summary"]["request_count"], 2);
    assert_eq!(dashboard["summary"]["error_count"], 1);
    assert_eq!(dashboard["requests"][0]["status"], 200);
    assert_eq!(dashboard["requests"][0]["failed"], true);
    let timeline = dashboard["timeline"].as_array().unwrap();
    assert!(timeline.len() <= 168);
    assert!(timeline.iter().any(|bin| bin["requests"] == 0));
    assert_eq!(
        timeline
            .iter()
            .map(|bin| bin["requests"].as_i64().unwrap())
            .sum::<i64>(),
        2
    );
    assert_eq!(
        timeline
            .iter()
            .map(|bin| bin["errors"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    let errors = Filter {
        status: Some("error".into()),
        ..filter.clone()
    };
    assert_eq!(
        store.dashboard(&errors).unwrap()["requests"][0]["id"],
        "body-aborted"
    );
    let detail = store.group("dashboard-group", &filter).unwrap().unwrap();
    assert_eq!(detail["group"]["error_count"], 1);
    assert_eq!(
        detail["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin["errors"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    let before = Filter {
        to: Some(now - 2_000),
        ..filter
    };
    assert_eq!(
        store.dashboard(&before).unwrap()["summary"]["request_count"],
        1
    );
}

#[test]
fn list_cursors_cover_tied_requests_and_groups_and_search_beyond_the_first_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("pages.sqlite"), 7, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..235)
        .map(|index| {
            let mut record = dashboard_record(&format!("request-{index:03}"), now - index % 3);
            record["answers"][0]["group_id"] = json!(format!("group-{index:03}"));
            record["answers"][0]["key"] = json!(format!("question-{index:03}"));
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let mut filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let mut requests = std::collections::HashSet::new();
    loop {
        let page = store.dashboard(&filter).unwrap();
        assert_eq!(page["summary"]["request_count"], 235);
        for request in page["requests"].as_array().unwrap() {
            assert!(requests.insert(text(request, "id")));
        }
        filter.request_cursor = page["feed_next_cursor"].as_str().map(str::to_owned);
        if filter.request_cursor.is_none() {
            break;
        }
    }
    assert_eq!(requests.len(), 235);
    let mut groups = std::collections::HashSet::new();
    loop {
        let page = store.dashboard(&filter).unwrap();
        for group in page["groups"].as_array().unwrap() {
            assert!(groups.insert(text(group, "id")));
        }
        filter.group_cursor = page["group_next_cursor"].as_str().map(str::to_owned);
        if filter.group_cursor.is_none() {
            break;
        }
    }
    assert_eq!(groups.len(), 235);
    filter.group_search = Some("question-233".into());
    let found = store.dashboard(&filter).unwrap();
    assert_eq!(found["groups"].as_array().unwrap().len(), 1);
    assert_eq!(found["groups"][0]["id"], "group-233");
    assert_eq!(found["summary"]["request_count"], 235);
}

#[test]
fn filtered_request_pages_bound_query_work_and_keep_small_selections_direct() {
    use rusqlite::StatementStatus;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("page-work.sqlite"), 7, 10_000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..4096)
        .map(|index| {
            let prefix = if index % 2 == 0 { "match" } else { "other" };
            // Groups of eight requests share timestamps; sequence must keep
            // ties stable when the cursor lands inside one of those groups.
            let sparse = if index % 2 == 0 && !(4..3896).contains(&index) {
                "sparse-"
            } else {
                ""
            };
            let mut record = dashboard_record(
                &format!("{prefix}-{sparse}{index:04}"),
                now - (4095 - index) / 8,
            );
            record["answers"] = json!([]);
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    for (search, broad, pages) in [
        ("match-", true, 2),
        // A selection just over one page, with its final two matches at the
        // oldest edge, must not scan all intervening nonmatching history.
        ("sparse-", false, 2),
        ("match-000", false, 1),
    ] {
        let mut connection = store.reader().unwrap();
        let transaction = connection.transaction().unwrap();
        let (predicate, args) = Store::dashboard_predicate(
            &transaction,
            &Filter {
                window: Some("all".into()),
                search: Some(search.into()),
                ..Default::default()
            },
        )
        .unwrap();
        let expected: Vec<_> = records
            .iter()
            .rev()
            .map(|record| text(record, "id"))
            .filter(|id| id.contains(search))
            .collect();
        let mut cursor = None;
        for page in 0..pages {
            let (query, values) = Store::request_summary_query(
                &transaction,
                &predicate,
                &args,
                101,
                cursor.as_deref(),
            )
            .unwrap();
            let mut statement = transaction.prepare(&query).unwrap();
            let ids = statement
                .query_map(params_from_iter(&values), |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let offset = page * 100;
            assert_eq!(ids, expected[offset..expected.len().min(offset + 101)]);
            if broad {
                assert_eq!(
                    statement.get_status(StatementStatus::Sort),
                    0,
                    "Broad filtered page {page} must not sort the full selection"
                );
            }
            // This is a deterministic SQLite work bound, not a wall-clock
            // benchmark. A small selection near the oldest history also needs
            // direct lookups instead of walking thousands of newer requests.
            let work = statement.get_status(StatementStatus::VmStep);
            assert!(work < 20_000, "{search} page {page} used {work} VM steps");

            let (summaries, next) =
                Store::request_page(&transaction, &predicate, &args, cursor.as_deref()).unwrap();
            assert_eq!(
                summaries
                    .iter()
                    .map(|record| text(record, "id"))
                    .collect::<Vec<_>>(),
                expected[offset..expected.len().min(offset + 100)]
            );
            assert_eq!(next.is_some(), expected.len() > offset + 100);
            cursor = next;
        }
    }
}

#[test]
fn version_metrics_count_parent_cost_once_and_keep_reviews_in_the_parent_scope() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("reviews.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut record = dashboard_record("reviewed", now);
    let mut second = record["answers"][0].clone();
    second["key"] = json!("second");
    second["warnings"] = json!(["Reported score differs from displayed probabilities"]);
    record["answers"].as_array_mut().unwrap().push(second);
    let mut unlabeled = record["answers"][0].clone();
    unlabeled["key"] = json!("unlabeled");
    unlabeled["warnings"] = json!([]);
    record["answers"].as_array_mut().unwrap().push(unlabeled);
    let mut other = dashboard_record("other-source", now);
    other["source"] = json!("outside-filter");
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[record, other])
        .unwrap();
    store.add_label("reviewed", "urgent", "correct").unwrap();
    store.add_label("reviewed", "second", "unknown").unwrap();
    store
        .add_label("other-source", "urgent", "incorrect")
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        source: Some("dashboard-test".into()),
        ..Default::default()
    };
    let detail = store.group("dashboard-group", &filter).unwrap().unwrap();
    let overview = store.dashboard(&filter).unwrap();
    tests::assert_overview_matches_detail(&overview["groups"][0], &detail["group"]);
    assert_eq!(detail["group"], detail["versions"][0]);
    for group in [&detail["group"], &detail["versions"][0]] {
        assert_eq!(
            group["review_counts"],
            json!({"correct":1,"incorrect":0,"unknown":1,"unlabeled":1})
        );
        assert_eq!(group["request_count"], 1);
        assert_eq!(group["cost_usd"], 0.25);
        assert_eq!(group["cost_known_requests"], 1);
        assert_eq!(group["input_tokens"], 100);
        assert_eq!(group["mean_latency_ms"], 4.0);
        assert_eq!(group["warning_count"], 1);
    }
    let empty = store
        .group(
            "dashboard-group",
            &Filter {
                source: Some("missing-source".into()),
                ..filter
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(empty["group"]["warning_count"], 0);
    assert_eq!(
        empty["group"]["review_counts"],
        json!({"correct":0,"incorrect":0,"unknown":0,"unlabeled":0})
    );
}

fn dashboard_record(id: &str, timestamp: i64) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "timestamp": timestamp,
        "source": "dashboard-test",
        "model": "test-model",
        "status": 200,
        "duration_ms": 4.0,
        "input_tokens": 100,
        "output_tokens": 3,
        "cost_usd": 0.25,
        "cost_basis": "synthetic",
        "capture_complete": true,
        "sample": false,
        "answers": [{
            "key": "urgent",
            "kind": "noul",
            "group_id": "dashboard-group",
            "definition_id": "definition",
            "presentation_id": "presentation",
            "definition": {"type": "noul", "instructions": "Urgent?"},
            "valid": true,
            "value": 0.8
        }]
    })
}

fn assert_snapshot_counts(snapshot: &Value, expected: i64) {
    assert_eq!(snapshot["summary"]["request_count"], expected);
    assert_eq!(snapshot["summary"]["answer_count"], expected);
    assert_eq!(
        snapshot["requests"].as_array().unwrap().len(),
        expected as usize
    );
    let group_requests: i64 = snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["request_count"].as_i64().unwrap())
        .sum();
    assert_eq!(group_requests, expected);
    let timeline_requests: i64 = snapshot["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["requests"].as_i64().unwrap())
        .sum();
    assert_eq!(timeline_requests, expected);
}

#[test]
fn dashboard_index_backfills_v1_history_without_inventing_import_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing-v1.sqlite");
    let now = Utc::now().timestamp_millis();
    let store = Store::open(&path, 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    // Reproduce a populated v1 database created before the dashboard index.
    writer
        .execute_batch("DROP INDEX request_dashboard")
        .unwrap();
    let timed = dashboard_record("timed", now - 3_000);
    let mut imported = dashboard_record("untimed-import", now);
    imported.as_object_mut().unwrap().remove("timestamp");
    imported["imported_at"] = json!(now - 2_000);
    imported["duration_ms"] = json!(8.0);
    imported["source_event_id"] = json!("original-event");
    imported["import_format"] = json!("observer-jsonl");
    let mut action = dashboard_record("untimed-action", now);
    action["timestamp"] = Value::Null;
    action["imported_at"] = json!(now - 1_000);
    action["event_kind"] = json!("action");
    action["answers"] = json!([]);
    store
        .write_batch(&mut writer, &[timed, imported, action])
        .unwrap();
    store
        .add_label("untimed-import", "urgent", "correct")
        .unwrap();
    let original = store.request("untimed-import").unwrap().unwrap();
    drop(writer);
    drop(store);

    let reopened = Store::open(&path, 7, 100).unwrap();
    let connection = reopened.reader().unwrap();
    let version: i64 = connection
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let index_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name='request_dashboard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1);
    assert_eq!(
        reopened.request("untimed-import").unwrap().unwrap(),
        original
    );

    let snapshot = reopened
        .dashboard(&Filter {
            window: Some("all".into()),
            as_of: Some(now),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(snapshot["summary"]["request_count"], 2);
    assert_eq!(snapshot["summary"]["answer_count"], 2);
    assert_eq!(snapshot["summary"]["action_count"], 1);
    assert_eq!(snapshot["summary"]["cost_usd"], 0.5);
    assert_eq!(snapshot["summary"]["p50_ms"], 4.0);
    assert_eq!(snapshot["summary"]["p95_ms"], 8.0);
    assert_eq!(snapshot["groups"][0]["request_count"], 2);
    assert_eq!(snapshot["groups"][0]["last_seen"], now - 2_000);
    assert_eq!(snapshot["requests"].as_array().unwrap().len(), 3);
    let timeline_requests: i64 = snapshot["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["requests"].as_i64().unwrap())
        .sum();
    assert_eq!(
        timeline_requests, 1,
        "Import time must not become provider event time"
    );
    let imported = snapshot["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["id"] == "untimed-import")
        .unwrap();
    assert!(imported["timestamp"].is_null());
    assert_eq!(imported["imported_at"], now - 2_000);
}

#[test]
fn dashboard_snapshots_follow_imports_retention_and_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("changing-history.sqlite"), 7, 2).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let now = Utc::now().timestamp_millis();
    let filter = Filter {
        window: Some("all".into()),
        search: Some("urgent".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let mut first = dashboard_record("first-import", now - 3_000);
    first["source_event_id"] = json!("original-event");
    first["import_format"] = json!("observer-jsonl");
    assert_eq!(store.write_batch(&mut writer, &[first.clone()]).unwrap(), 1);
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 1);

    let mut duplicate = first;
    duplicate["id"] = json!("duplicate-import");
    let second = dashboard_record("second", now - 2_000);
    assert_eq!(
        store
            .write_batch(&mut writer, &[duplicate, second])
            .unwrap(),
        1
    );
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 2);

    let mut third = dashboard_record("third", now - 1_000);
    third["answers"][0]["value"] = json!(0.2);
    store.write_batch(&mut writer, &[third]).unwrap();
    store.maintenance(&mut writer, true).unwrap();
    let retained = store.dashboard(&filter).unwrap();
    assert_snapshot_counts(&retained, 2);
    assert_eq!(retained["summary"]["cost_usd"], 0.5);
    assert!(store.request("first-import").unwrap().is_none());
    assert_eq!(
        retained["groups"][0]["distribution"],
        json!([
            {"label": "0.2–<0.3", "count": 1},
            {"label": "0.8–<0.9", "count": 1}
        ])
    );

    store.delete_all().unwrap();
    let empty = store.dashboard(&filter).unwrap();
    assert_snapshot_counts(&empty, 0);
    assert!(empty["summary"]["cost_usd"].is_null());
    assert!(empty["summary"]["p50_ms"].is_null());
    assert!(empty["sources"].as_array().unwrap().is_empty());
    assert!(empty["models"].as_array().unwrap().is_empty());

    store
        .write_batch(&mut writer, &[dashboard_record("after-delete", now)])
        .unwrap();
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 1);
}

fn sql_request_aggregates(
    store: &Store,
    filter: &Filter,
    timeline_meta: &Value,
) -> (Value, Vec<Value>) {
    let connection = store.reader().unwrap();
    let (predicate, args) = Store::predicate(filter).unwrap();
    let mut summary = connection.query_row(
        &format!("SELECT count(*),coalesce(sum(answer_count),0),coalesce(sum(status>=400),0),sum(input_tokens),sum(output_tokens),sum(cost_usd),count(cost_usd),coalesce(sum(sample),0),coalesce(sum(NOT capture_complete),0),count(input_tokens),count(output_tokens) FROM requests r WHERE {predicate} AND event_kind='request'"),
        params_from_iter(&args),
        |row| Ok(json!({
            "request_count": row.get::<_, i64>(0)?,
            "answer_count": row.get::<_, i64>(1)?,
            "error_count": row.get::<_, i64>(2)?,
            "input_tokens": row.get::<_, Option<i64>>(3)?,
            "output_tokens": row.get::<_, Option<i64>>(4)?,
            "cost_usd": row.get::<_, Option<f64>>(5)?,
            "cost_known_requests": row.get::<_, i64>(6)?,
            "sample_count": row.get::<_, i64>(7)?,
            "incomplete_count": row.get::<_, i64>(8)?,
            "input_tokens_known_requests": row.get::<_, i64>(9)?,
            "output_tokens_known_requests": row.get::<_, i64>(10)?,
            "input_tokens_overflow": false,
            "output_tokens_overflow": false
        })),
    ).unwrap();
    summary["action_count"] =
        json!(connection.query_row(
        &format!("SELECT count(*) FROM requests r WHERE {predicate} AND event_kind<>'request'"),
        params_from_iter(&args),
        |row| row.get::<_, i64>(0),
    ).unwrap());
    let durations: Vec<f64> = connection.prepare(
        &format!("SELECT duration_ms FROM requests r WHERE {predicate} AND event_kind='request' AND duration_ms IS NOT NULL ORDER BY duration_ms"),
    ).unwrap().query_map(params_from_iter(&args), |row| row.get(0))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap();
    for (name, numerator) in [("p50_ms", 50), ("p95_ms", 95)] {
        summary[name] = if durations.is_empty() {
            Value::Null
        } else {
            // Integer nearest rank is independent of the production selection.
            json!(durations[(durations.len() * numerator).div_ceil(100) - 1])
        };
    }
    // SQL independently aggregates every record into the response's declared
    // bins. Separate tests assert full-range coverage, empty bins and bounds.
    let width = timeline_meta["bucket_width"].as_i64().unwrap();
    let start = timeline_meta["start"].as_i64().unwrap();
    let end = timeline_meta["end"].as_i64().unwrap() - 1;
    let mut timeline: BTreeMap<i64, Value> = connection.prepare(
        &format!("SELECT ((timestamp-{start})/{width})*{width}+{start},count(*),coalesce(sum({FAILED}),0),avg(duration_ms),sum(cost_usd) FROM requests r WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL GROUP BY 1 ORDER BY 1"),
    ).unwrap().query_map(params_from_iter(&args), |row| Ok((row.get::<_, i64>(0)?, json!({
        "timestamp": row.get::<_, i64>(0)?,
        "requests": row.get::<_, i64>(1)?,
        "errors": row.get::<_, i64>(2)?,
        "mean_latency_ms": row.get::<_, Option<f64>>(3)?,
        "cost_usd": row.get::<_, Option<f64>>(4)?
    })))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    let timeline = (start..=end)
        .step_by(width as usize)
        .map(|timestamp| {
            timeline.remove(&timestamp).unwrap_or_else(|| json!({
        "timestamp":timestamp,"requests":0,"errors":0,"mean_latency_ms":null,"cost_usd":null
    }))
        })
        .collect();
    (summary, timeline)
}

#[test]
fn request_aggregates_match_sql_across_filters_unknowns_and_long_timelines() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("aggregate-reference.sqlite"), 30, 2_000).unwrap();
    let hour = 3_600_000;
    let anchor = Utc::now().timestamp_millis() / hour * hour - hour;
    let as_of = anchor + hour / 2;
    let mut records = Vec::new();
    // A coprime permutation deliberately separates insertion and timestamp
    // order. Search filters also exercise materialized parent selections.
    for ordinal in 0..193 {
        let bucket = ordinal * 73 % 193;
        for within in [2, 0, 1] {
            let number = bucket * 3 + within;
            let timestamp = anchor - bucket * hour + within * 10_000;
            let mut record = dashboard_record(&format!("record-{bucket:03}-{within}"), timestamp);
            record["source"] = json!(if bucket % 2 == 0 { "alpha" } else { "beta" });
            record["model"] = json!(if within == 1 { "model-b" } else { "model-a" });
            record["status"] = match number % 7 {
                0 => Value::Null,
                1 | 2 => json!(503),
                _ => json!(200),
            };
            record["duration_ms"] = if number % 5 == 0 {
                Value::Null
            } else {
                json!([0.1, 0.2, 0.3, 0.0, 4.125, 14.75, 101.333][number as usize % 7])
            };
            record["cost_usd"] = if number % 4 == 0 {
                Value::Null
            } else {
                json!([0.1, 0.01, 0.2, 0.0001][number as usize % 4])
            };
            record["input_tokens"] = if number % 4 == 0 {
                Value::Null
            } else {
                json!(number % 19)
            };
            record["output_tokens"] = if number % 3 == 0 {
                Value::Null
            } else {
                json!(number % 13)
            };
            record["sample"] = json!(number % 9 == 0);
            record["capture_complete"] = json!(number % 6 != 0);
            record["answers"][0]["group_id"] = json!(if bucket % 2 == 0 {
                "alpha-group"
            } else {
                "beta-group"
            });
            if within == 1 && bucket % 4 == 0 {
                let mut companion = record["answers"][0].clone();
                companion["key"] = json!("companion");
                companion["group_id"] = json!("companion-group");
                record["answers"].as_array_mut().unwrap().push(companion);
            }
            if number % 17 == 0 {
                record["timestamp"] = Value::Null;
                record["imported_at"] = json!(timestamp);
                if within == 1 {
                    record.as_object_mut().unwrap().remove("timestamp");
                }
            }
            if within == 2 && bucket % 7 == 0 {
                record["event_kind"] = json!("action");
                // Actions deliberately contain tempting metrics that must not
                // enter request totals, percentiles, or timeline averages.
                record["duration_ms"] = json!(999_999.0);
                record["cost_usd"] = json!(999.0);
                record["answers"] = json!([]);
            }
            records.push(record);
        }
    }
    records.push(dashboard_record("future", as_of + 1));
    let mut unknown = dashboard_record("unknown", anchor + 1);
    unknown["source"] = json!("unknown-only");
    for key in ["duration_ms", "cost_usd", "input_tokens", "output_tokens"] {
        unknown[key] = Value::Null;
    }
    records.push(unknown);
    let mut action = dashboard_record("action-only", anchor + 2);
    action["source"] = json!("actions-only");
    action["event_kind"] = json!("action");
    records.push(action);
    let mut source_match = dashboard_record("source-match", anchor + 3);
    source_match["source"] = json!("MiXeD-SeArCh archive");
    records.push(source_match);
    records.push(dashboard_record("mixed-search-id", anchor + 3));
    let mut key_match = dashboard_record("key-match", anchor + 3);
    let mut companion = key_match["answers"][0].clone();
    key_match["answers"][0]["key"] = json!("mixed-search-key");
    key_match["answers"][0]["group_id"] = json!("mixed-search-group");
    companion["key"] = json!("unrelated-companion");
    companion["group_id"] = json!("mixed-companion-group");
    key_match["answers"].as_array_mut().unwrap().push(companion);
    records.push(key_match);
    assert!(
        records.len() > 512,
        "Broad searches must exercise batched answer-key selection"
    );
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    let cases = [
        json!({"window": "all"}),
        json!({"window": "all", "search": "record"}),
        json!({"window": "all", "search": "urgent"}),
        json!({"window": "all", "search": "MiXeD-SeArCh"}),
        json!({"window": "all", "source": "alpha"}),
        json!({"window": "all", "model": "model-b"}),
        json!({"window": "all", "status": "error"}),
        json!({"window": "all", "group": "alpha-group"}),
        json!({"window": "all", "search": "companion"}),
        json!({"window": "all", "search": "record-192-1"}),
        json!({"window": "all", "search": "absent"}),
        json!({"window": "all", "source": "unknown-only"}),
        json!({"window": "all", "source": "actions-only"}),
        json!({"window": "1h"}),
        json!({"window": "24h"}),
        json!({"window": "7d"}),
        json!({"window": "24h", "source": "alpha", "model": "model-b", "status": "error", "search": "urgent"}),
        json!({}),
    ];
    for case in cases {
        let mut filter: Filter = serde_json::from_value(case.clone()).unwrap();
        filter.as_of = Some(as_of);
        let snapshot = store.dashboard(&filter).unwrap();
        let (summary, timeline) =
            sql_request_aggregates(&store, &filter, &snapshot["timeline_meta"]);
        assert_eq!(snapshot["summary"], summary, "summary for {case}");
        assert_eq!(snapshot["timeline"], json!(timeline), "timeline for {case}");
        let (predicate, args) = Store::predicate(&filter).unwrap();
        let feed =
            Store::request_summaries(&store.reader().unwrap(), &predicate, &args, 100).unwrap();
        assert_eq!(
            snapshot["requests"],
            json!(feed),
            "newest-first feed for {case}"
        );
        if case["search"] == "MiXeD-SeArCh" {
            assert_eq!(snapshot["summary"]["request_count"], 3);
            assert_eq!(snapshot["summary"]["answer_count"], 4);
            let companion = snapshot["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|group| group["id"] == "mixed-companion-group")
                .unwrap();
            assert_eq!(
                companion["answer_count"], 1,
                "A key match must retain the parent's other answers"
            );
            assert_eq!(snapshot["requests"][0]["id"], "key-match");
            assert_eq!(snapshot["requests"][1]["id"], "mixed-search-id");
            assert_eq!(snapshot["requests"][2]["id"], "source-match");
        }
        if case == json!({"window": "all"}) {
            assert!(snapshot["timeline"].as_array().unwrap().len() <= 168);
            assert_eq!(snapshot["timeline_meta"]["start"], anchor - 192 * hour);
            assert_eq!(snapshot["timeline_meta"]["end"], as_of + 2);
            assert_eq!(snapshot["timeline_meta"]["truncated"], false);
        }
    }
}

#[test]
fn request_aggregates_preserve_small_fractional_charges_next_to_large_values() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("compensated-metrics.sqlite"), 7, 2_000).unwrap();
    let timestamp = Utc::now().timestamp_millis();
    let mut records = Vec::new();
    for ordinal in 0..1_001 {
        let mut record = dashboard_record(&format!("fraction-{ordinal:04}"), timestamp);
        let value = if ordinal == 0 { 1e12 } else { 0.1 };
        record["cost_usd"] = json!(value);
        record["duration_ms"] = json!(value);
        records.push(record);
    }
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let snapshot = store.dashboard(&filter).unwrap();
    let (summary, timeline) = sql_request_aggregates(&store, &filter, &snapshot["timeline_meta"]);
    assert_eq!(snapshot["summary"], summary);
    assert_eq!(snapshot["timeline"], json!(timeline));
    // An ordinary left-to-right floating sum loses about 0.024 here.
    assert_eq!(snapshot["summary"]["cost_usd"], json!(1_000_000_000_100.0));
    assert_eq!(
        snapshot["timeline"][0]["cost_usd"],
        json!(1_000_000_000_100.0)
    );
    assert_eq!(
        snapshot["timeline"][0]["mean_latency_ms"],
        json!(1_000_000_000_100.0 / 1_001.0)
    );
    assert_eq!(snapshot["summary"]["p50_ms"], 0.1);
    assert_eq!(snapshot["summary"]["p95_ms"], 0.1);
}

#[test]
#[ignore = "bounded CSV export timing; run explicitly with --ignored --nocapture"]
fn csv_export_large_definitions_benchmark() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("csv-definitions.sqlite"), 36_500, 1_000).unwrap();
    let records: Vec<_> = (0..200)
        .map(|ordinal| {
            let mut record = dashboard_record(&format!("csv-{ordinal:03}"), 1_790_000_000_000);
            let answers: Vec<_> = (0..10)
                .map(|key| {
                    let mut answer = record["answers"][0].clone();
                    answer["key"] = json!(format!("key-{key}"));
                    answer["group_id"] = json!(format!("group-{key}"));
                    answer["definition"]["instructions"] = json!("x".repeat(8 * 1024));
                    answer
                })
                .collect();
            record["answers"] = json!(answers);
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let expected = store.export(&filter, "csv").unwrap();
    assert_eq!(expected.lines().count(), 201);
    let mut elapsed = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let csv = store.export(&filter, "csv").unwrap();
        elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(csv, expected);
    }
    use sha2::{Digest, Sha256};
    println!(
        "{}",
        json!({
            "fixture": {"requests": 200, "answers_per_request": 10, "definition_bytes": 8_192},
            "csv_bytes": expected.len(),
            "csv_sha256": format!("{:x}", Sha256::digest(expected.as_bytes())),
            "elapsed_ms": elapsed,
        })
    );
}

#[test]
fn version_counts_preserve_sources_and_families_with_bounded_lookup_work() {
    use rusqlite::StatementStatus;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("version-counts.sqlite"), 7, 2_000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut writer = store.writer_connection().unwrap();
    let group_record = |id: &str, source: &str, key: &str, family: Option<&str>| {
        let mut record = dashboard_record(id, now);
        record["source"] = json!(source);
        record["answers"][0]["key"] = json!(key);
        record["answers"][0]["group_id"] = json!(id);
        record["answers"][0]["definition_id"] = json!(format!("definition-{id}"));
        record["answers"][0]["family_id"] = json!(family);
        record["answers"][0]["family_name"] = json!(family);
        record
    };
    store
        .write_batch(
            &mut writer,
            &[
                group_record("a0", "source-a", "question", Some("family-a")),
                group_record("a1", "source-a", "question", None),
                group_record("a2", "source-a", "question", Some("other-family-a")),
                group_record("a3", "source-a", "another-key", Some("family-a")),
                group_record("b0", "source-b", "question", Some("family-b")),
                group_record("b1", "source-b", "question", None),
            ],
        )
        .unwrap();
    let all = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    for (source, expected) in [
        (
            "source-a",
            vec![
                ("a0", 3, false),
                ("a1", 3, true),
                ("a2", 3, false),
                ("a3", 1, false),
                ("family-a", 2, true),
                ("other-family-a", 1, true),
            ],
        ),
        (
            "source-b",
            vec![("b0", 2, false), ("b1", 2, true), ("family-b", 1, true)],
        ),
    ] {
        let filter = Filter {
            source: Some(source.into()),
            ..all.clone()
        };
        let dashboard = store.dashboard(&filter).unwrap();
        assert_eq!(
            dashboard["groups"].as_array().unwrap().len(),
            expected.iter().filter(|(_, _, visible)| *visible).count()
        );
        for (id, count, visible) in expected {
            // Family members pool into one overview entry, but each strict
            // member still has its own detail view and complete version list.
            let overview = dashboard["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|group| group["id"] == id);
            assert_eq!(overview.is_some(), visible, "{id}");
            if let Some(overview) = overview {
                assert_eq!(overview["source"], source);
                assert_eq!(overview["version_count"], count, "{id}");
            }
            let detail = store.group(id, &filter).unwrap().unwrap();
            assert_eq!(detail["group"]["version_count"], count, "{id}");
            assert_eq!(
                detail["versions"].as_array().unwrap().len(),
                count as usize,
                "{id}"
            );
        }
    }
    // Most groups share the source but not the key or family. Counting a few
    // versions must not visit this unrelated population for each visible group.
    let unrelated: Vec<_> = (0..1024)
        .map(|index| {
            group_record(
                &format!("unrelated-{index:04}"),
                "source-a",
                &format!("other-key-{index:04}"),
                None,
            )
        })
        .collect();
    store.write_batch(&mut writer, &unrelated).unwrap();
    for (family, source, selector, expected) in [
        (false, "source-a", "question", 3),
        (false, "source-b", "question", 2),
        (false, "source-a", "missing", 0),
        (true, "source-a", "family-a", 2),
        (true, "source-a", "other-family-a", 1),
        (true, "source-b", "family-a", 0),
        (true, "source-b", "family-b", 1),
    ] {
        let mut stmt = writer.prepare(Store::version_count_query(family)).unwrap();
        let count: i64 = stmt
            .query_row(params![source, selector], |row| row.get(0))
            .unwrap();
        assert_eq!(count, expected);
        assert!(
            stmt.get_status(StatementStatus::VmStep) < 1000,
            "Version lookup must stay bounded by matching key/family, not all groups from {source}"
        );
    }
}

#[test]
fn malformed_capture_definitions_survive_stored_jsonl_export_and_reimport() {
    use crate::model::{Capture, NormalizeOptions, import_records, normalize};

    let options = NormalizeOptions::default();
    let now = Utc::now().timestamp_millis();
    let capture = Capture {
        id: "live-malformed".into(),
        timestamp: now,
        source: "malformed-fixture".into(),
        status: 200,
        capture_complete: true,
        request: serde_json::to_vec(&json!({"questions": {"q": 42}})).unwrap(),
        response: serde_json::to_vec(&json!({"answers": {"q": {"type": "noul", "noul": 0.8}}}))
            .unwrap(),
        ..Default::default()
    };
    let envelope = json!({
        "id": "imported-malformed", "timestamp": now, "source": "malformed-fixture",
        "provider": "typesafe", "status": 200, "capture_complete": true,
        "request": {"questions": {"q": [null, 1]}},
        "response": {"answers": {"q": {"type": "noul", "noul": 0.8}}}
    });
    let records = vec![
        normalize(&capture, &options),
        import_records(&envelope.to_string(), "systemone-capture", &options)
            .unwrap()
            .remove(0),
    ];
    let dir = tempfile::tempdir().unwrap();
    let original = Store::open(&dir.path().join("original.sqlite"), 7, 100).unwrap();
    let restored = Store::open(&dir.path().join("restored.sqlite"), 7, 100).unwrap();
    original
        .write_batch(&mut original.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let exported = original.export(&filter, "jsonl").unwrap();
    let imported = import_records(&exported, "observer-jsonl", &options).unwrap();
    assert_eq!(imported.len(), 2);
    for record in &imported {
        let before = records
            .iter()
            .find(|before| before["id"] == record["id"])
            .unwrap();
        assert_eq!(record["answers"], before["answers"]);
        assert_eq!(record["answers"][0]["valid"], false);
    }
    let mut writer = restored.writer_connection().unwrap();
    assert_eq!(restored.write_batch(&mut writer, &imported).unwrap(), 2);
    let again = import_records(
        &restored.export(&filter, "jsonl").unwrap(),
        "observer-jsonl",
        &options,
    )
    .unwrap();
    assert_eq!(restored.write_batch(&mut writer, &again).unwrap(), 0);
    for record in again {
        let before = imported
            .iter()
            .find(|before| before["id"] == record["id"])
            .unwrap();
        assert_eq!(record["answers"], before["answers"]);
        assert_eq!(record["source_event_id"], before["source_event_id"]);
    }
    let dashboard = restored.dashboard(&filter).unwrap();
    assert_eq!(dashboard["summary"]["request_count"], 2);
    assert_eq!(dashboard["groups"].as_array().unwrap().len(), 2);
}

#[test]
fn compact_answer_index_preserves_ids_and_duplicate_parent_order() {
    use rusqlite::{StatementStatus, types::Value as SqlValue};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("compact.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let canonical = format!("group_v1_{}", "ab".repeat(32));
    let ids = [
        canonical.clone(),
        format!("group_v1_{}", "AB".repeat(32)),
        format!("{canonical}\0tail"),
        format!("group_v1_{}\0", "a".repeat(63)),
        format!("group_v1_{}g", "a".repeat(63)),
        "plain-id".into(),
        "".into(),
        format!("group_v1_{}", "é".repeat(64)),
        format!("group_v1_{}", "00".repeat(32)),
    ];
    let mut writer = store.writer_connection().unwrap();
    for (i, id) in ids.iter().enumerate() {
        let mut record = dashboard_record(&format!("parent-{i}"), now);
        let original = record["answers"][0].clone();
        record["answers"] = json!([]);
        for (n, value) in [1e16, 1.0, -1e16, 0.25].into_iter().enumerate() {
            let mut answer = original.clone();
            answer["key"] = json!(format!("key-{n}"));
            answer["group_id"] = json!(id);
            // Imported groups may reuse an ID with inconsistent kinds; the
            // compact index must preserve the previous tie order even here.
            answer["kind"] = json!(if n % 2 == 0 { "noul" } else { "number" });
            answer["value"] = json!(value);
            record["answers"].as_array_mut().unwrap().push(answer);
        }
        store.write_batch(&mut writer, &[record]).unwrap();
    }
    // These inserts use the original columns, as an older writer does. The
    // expression index is maintained by SQLite without app-specific functions.
    writer
        .execute("UPDATE answers SET confidence=0.7 WHERE key='key-1'", [])
        .unwrap();
    for id in &ids {
        let mut previous = writer.prepare("SELECT request_id,valid,value_num,confidence,value_text FROM answers INDEXED BY answer_statistics WHERE group_id=? ORDER BY request_id").unwrap();
        let mut compact = writer.prepare(super::STRICT_ANSWER_QUERY).unwrap();
        let collect = |stmt: &mut rusqlite::Statement<'_>| {
            stmt.query_map([id], |row| {
                (0..5)
                    .map(|i| row.get::<_, SqlValue>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        let expected = collect(&mut previous);
        assert_eq!(expected.len(), 4, "{id:?}");
        assert_eq!(collect(&mut compact), expected, "{id:?}");
        assert_eq!(compact.get_status(StatementStatus::Sort), 0);
        assert_eq!(compact.get_status(StatementStatus::FullscanStep), 0);
    }
    writer
        .execute("DELETE FROM requests WHERE id='parent-0'", [])
        .unwrap();
    assert!(
        !writer
            .query_row(super::STRICT_ANSWER_QUERY, [&canonical], |_| Ok(true))
            .optional()
            .unwrap()
            .unwrap_or(false)
    );
}

#[test]
fn answer_key_discovery_skips_duplicates_and_bounds_high_cardinality() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("keys.sqlite"), 7, 20_000).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let now = Utc::now().timestamp_millis();
    let mut template = dashboard_record("template", now);
    template["answers"][0]["key"] = json!("UrGency_%");
    store.write_batch(&mut writer, &[template]).unwrap();
    writer.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO answers SELECT request_id,'repeat-'||x,group_id,kind,valid,value_num,value_text,confidence,data FROM answers CROSS JOIN n WHERE key='UrGency_%'").unwrap();
    // 513 distinct keys proves the bounded dictionary is incomplete. Never
    // silently omit the matching answer in the original scan fallback.
    assert!(
        Store::dashboard_answer_keys(&writer, "urgency")
            .unwrap()
            .is_none()
    );
    writer
        .execute("DELETE FROM answers WHERE key LIKE 'repeat-%'", [])
        .unwrap();
    assert_eq!(
        Store::dashboard_answer_keys(&writer, "urgency").unwrap(),
        Some(vec!["UrGency_%".into()])
    );
    assert_eq!(
        Store::dashboard_answer_keys(&writer, "_%").unwrap(),
        Some(vec!["UrGency_%".into()])
    );
    assert_eq!(
        Store::dashboard_answer_keys(&writer, "missing").unwrap(),
        Some(vec![])
    );
    writer
        .execute(
            "UPDATE answers SET key=?",
            [format!("large-{}", "x".repeat(128 * 1024))],
        )
        .unwrap();
    assert!(
        Store::dashboard_answer_keys(&writer, "large")
            .unwrap()
            .is_none()
    );
    writer.execute("DELETE FROM answers", []).unwrap();
    assert_eq!(
        Store::dashboard_answer_keys(&writer, "x").unwrap(),
        Some(vec![])
    );
}

#[test]
fn repeated_answer_keys_reduce_search_work_without_changing_parent_selection() {
    use rusqlite::StatementStatus;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("key-search.sqlite"), 7, 3000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..2000)
        .map(|i| {
            let mut record = dashboard_record(&format!("parent-{i}"), now - i);
            let answer = record["answers"][0].clone();
            record["answers"] = json!([]);
            for key in ["UrGency_%", "sentiment", "category"] {
                let mut answer = answer.clone();
                answer["key"] = json!(key);
                record["answers"].as_array_mut().unwrap().push(answer);
            }
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let conn = store.reader().unwrap();
    for search in ["urgency", "_%", "missing", "parent-12"] {
        let keys = Store::dashboard_answer_keys(&conn, search)
            .unwrap()
            .unwrap();
        let filter = Filter {
            window: Some("all".into()),
            search: Some(search.into()),
            ..Default::default()
        };
        let query = |keys: Option<&[String]>| {
            let (predicate, args) = Store::request_predicate(&filter, true, keys).unwrap();
            let mut stmt = conn.prepare(&format!("SELECT r.id FROM requests r INDEXED BY request_join_scope WHERE {predicate} ORDER BY r.id")).unwrap();
            let ids = stmt
                .query_map(params_from_iter(&args), |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (ids, stmt.get_status(StatementStatus::VmStep))
        };
        let (expected, before) = query(None);
        let (actual, after) = query(Some(&keys));
        assert_eq!(actual, expected, "{search}");
        assert!(after < before * 9 / 10, "{search}: {before} -> {after}");
    }
    let (_, work) = measured_overview(
        &store,
        &Filter {
            window: Some("all".into()),
            search: Some("urgency".into()),
            ..Default::default()
        },
    );
    assert!(
        work.answer_keys > 0 && work.answer_keys < 1000,
        "{}",
        work.answer_keys
    );
    let writer = store.writer_connection().unwrap();
    writer
        .execute("UPDATE answers SET key=key || request_id", [])
        .unwrap();
    assert!(
        Store::dashboard_answer_keys(&writer, "urgency")
            .unwrap()
            .is_none()
    );
    let (snapshot, work) = measured_overview(
        &store,
        &Filter {
            window: Some("all".into()),
            search: Some("urgency".into()),
            ..Default::default()
        },
    );
    assert_eq!(snapshot.unwrap()["summary"]["request_count"], 2000);
    assert!(
        work.answer_keys > 0 && work.answer_keys < 30_000,
        "{}",
        work.answer_keys
    );
}
