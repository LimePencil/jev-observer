use super::*;
use crate::model::{Capture, NormalizeOptions, normalize};

fn captured_record(id: &str, instructions: usize, extension: usize, model: &str) -> Value {
    let capture = Capture {
        id: id.into(),
        timestamp: Utc::now().timestamp_millis(),
        source: "s".repeat(256),
        task_version: None,
        adapter: None,
        status: 200,
        duration_ms: 1.0,
        request: serde_json::to_vec(&json!({"questions":{"q":{
            "type":"noul", "instructions":"x".repeat(instructions)
        }}}))
        .unwrap(),
        response: serde_json::to_vec(&json!({"model":model, "answers":{"q":{
            "type":"noul", "noul":0.5, "extension":"y".repeat(extension)
        }}}))
        .unwrap(),
        capture_complete: true,
        transport_error: None,
        secret: None,
        local_token: None,
        access_token: None,
    };
    // These records can come from live captures at the supported body limit.
    assert!(capture.request.len() <= 16 * 1024 * 1024);
    assert!(capture.response.len() <= 16 * 1024 * 1024);
    let record = normalize(&capture, &NormalizeOptions::default());
    assert_eq!(record["answers"][0]["valid"], true);
    record
}

fn all_history() -> Filter {
    Filter {
        window: Some("all".into()),
        ..Default::default()
    }
}

/// The previous selection semantics, evaluated once in sequence order. Share
/// only formatting with the paged implementation so complete output bytes can
/// be checked independently of its temporary selection and cursor handling.
fn reference_export(store: &Store, filter: &Filter, format: &str) -> String {
    let mut connection = store.reader().unwrap();
    let transaction = connection.transaction().unwrap();
    let (predicate, args) = Store::predicate(filter).unwrap();
    let ids = transaction
        .prepare(&format!(
            "SELECT id,seq FROM requests r WHERE {predicate} AND seq>0 ORDER BY seq"
        ))
        .unwrap()
        .query_map(params_from_iter(&args), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    Store::export_page_from(
        &transaction,
        format,
        0,
        i64::MAX,
        ids,
        usize::MAX,
        usize::MAX,
    )
    .unwrap()
    .0
}

#[derive(Default, Debug)]
struct ExportWork {
    selection_steps: i64,
    selection_statements: usize,
    page_steps: i64,
    page_statements: usize,
    largest_page_steps: i64,
    sorts: i64,
    body_queries: usize,
}

fn measured_export(store: &Store, filter: &Filter, format: &str) -> (String, ExportWork) {
    unsafe extern "C" fn count_work(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        unsafe {
            let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
            let sql = std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement)).to_bytes();
            let work = &mut *context.cast::<ExportWork>();
            let steps = i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                1,
            ));
            if sql.starts_with(b"INSERT INTO temp.export_request_ids") {
                work.selection_steps += steps;
                work.selection_statements += 1;
            }
            if sql.starts_with(b"SELECT seq FROM temp.export_request_ids") {
                work.page_steps += steps;
                work.page_statements += 1;
                work.largest_page_steps = work.largest_page_steps.max(steps);
            }
            work.sorts += i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_SORT,
                1,
            ));
            if sql.starts_with(b"SELECT data,") || sql.starts_with(b"SELECT a.data,") {
                work.body_queries += 1;
            }
        }
        0
    }

    // The boxed callback context outlives the connection, including unwinding.
    // Connection destruction removes the trace before the context is dropped.
    let mut work = Box::<ExportWork>::default();
    let connection = store.reader().unwrap();
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            connection.handle(),
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(count_work),
            std::ptr::from_mut(work.as_mut()).cast(),
        );
    }
    let mut session = store.start_export_from(connection, filter, format).unwrap();
    assert!(session.selection.is_some());
    let mut output = String::new();
    loop {
        let (page, _, _, more) = session.next_page().unwrap();
        output.push_str(&page);
        if !more {
            break;
        }
    }
    drop(session);
    (output, *work)
}

#[test]
fn filtered_export_pages_do_linear_work_for_dense_and_sparse_sources_and_models() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("selection-work.sqlite"), 7, 5000).unwrap();
    let mut base = captured_record("fixture", 0, 0, "dense");
    base["answers"] = json!([]);
    let records: Vec<_> = (0..4096)
        .map(|index| {
            let mut record = base.clone();
            record["id"] = json!(format!("row-{index:04}"));
            // Reverse timestamps make index order disagree with sequence.
            record["timestamp"] = json!(base["timestamp"].as_i64().unwrap() - index);
            let scope = if index % 17 == 0 { "sparse" } else { "dense" };
            record["source"] = json!(scope);
            record["model"] = json!(scope);
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    for scope in ["dense", "sparse"] {
        for by_source in [true, false] {
            let filter = Filter {
                source: by_source.then(|| scope.into()),
                model: (!by_source).then(|| scope.into()),
                ..all_history()
            };
            for format in ["jsonl", "csv"] {
                let (actual, work) = measured_export(&store, &filter, format);
                assert_eq!(actual, reference_export(&store, &filter, format));
                let count = records
                    .iter()
                    .filter(|record| record["source"] == scope)
                    .count();
                assert_eq!(work.selection_statements, 1, "{work:?}");
                assert_eq!(work.page_statements, count / 100 + 1, "{work:?}");
                assert_eq!(work.sorts, 0, "{work:?}");
                assert_eq!(work.body_queries, 0, "{work:?}");
                assert!(work.largest_page_steps < 1000, "{work:?}");
                assert!(
                    work.selection_steps + work.page_steps < count as i64 * 40 + 1000,
                    "{work:?}"
                );
            }
        }
    }
}

#[test]
fn filtered_exports_preserve_mixed_predicates_exact_jsonl_and_escaped_csv() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("mixed-export.sqlite"), 7, 1000).unwrap();
    let base = captured_record("fixture", 0, 0, "=model,\"quoted\"\nvalue");
    let now = base["timestamp"].as_i64().unwrap();
    let group = text(&base["answers"][0], "group_id");
    let records: Vec<_> = (0..605)
        .map(|index| {
            let mut record = base.clone();
            record["id"] = json!(format!("mixed-{index:04}"));
            record["timestamp"] = json!(now - index / 3);
            record["source"] = json!(if index % 7 == 0 { "other" } else { "selected" });
            if index % 3 == 0 {
                record["status"] = json!(503);
            } else if index % 3 == 1 {
                record["transport_error"] = json!("synthetic failure");
            }
            if index % 11 == 0 {
                record["answers"] = json!([]);
            }
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let common = Filter {
        source: Some("selected".into()),
        model: Some(text(&base, "model")),
        as_of: Some(now),
        ..all_history()
    };
    for filter in [
        common.clone(),
        Filter {
            status: Some("error".into()),
            ..common.clone()
        },
        Filter {
            group: Some(group.clone()),
            ..common.clone()
        },
        Filter {
            search: Some("q".into()),
            ..common.clone()
        },
        Filter {
            status: Some("error".into()),
            group: Some(group),
            search: Some("q".into()),
            from: Some(now - 150),
            to: Some(now - 5),
            ..common.clone()
        },
        Filter {
            search: Some("no-such-request".into()),
            ..common
        },
    ] {
        for format in ["jsonl", "csv"] {
            let (actual, work) = measured_export(&store, &filter, format);
            assert_eq!(actual, reference_export(&store, &filter, format));
            assert_eq!(work.selection_statements, 1);
            assert_eq!(
                work.sorts, 0,
                "Mixed predicates must not spool source JSON: {work:?}"
            );
            assert_eq!(work.body_queries, 0);
        }
    }
}

#[test]
fn selected_exports_skip_deleted_pages_exclude_reinsertions_and_release_wal_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("changes-export.sqlite"), 7, 1000).unwrap();
    let mut base = captured_record("fixture", 0, 0, "selected-model");
    base["source"] = json!("selected");
    let records: Vec<_> = (1..=305)
        .map(|index| {
            let mut record = base.clone();
            record["id"] = json!(format!("row-{index:03}"));
            record
        })
        .collect();
    let mut writer = store.writer_connection().unwrap();
    store.write_batch(&mut writer, &records).unwrap();
    store.add_label("row-202", "q", "correct").unwrap();
    let filter = Filter {
        source: Some("selected".into()),
        ..all_history()
    };
    let mut session = store.start_export(&filter, "jsonl").unwrap();
    let selection = session.selection.as_ref().unwrap();
    let columns = selection
        .prepare("PRAGMA temp.table_info(export_request_ids)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(columns, [("seq".into(), "INTEGER".into())]);
    assert!(selection.is_autocommit());
    let (first, cursor, through, more) = session.next_page().unwrap();
    assert_eq!((cursor, through, more), (100, 305, true));
    assert_eq!(first.lines().count(), 100);

    // Delete an entire selected page, then replace a later ID with a new seq.
    writer
        .execute("DELETE FROM requests WHERE seq BETWEEN 101 AND 201", [])
        .unwrap();
    store
        .write_batch(&mut writer, &[records[200].clone()])
        .unwrap();
    let mut late = base;
    late["id"] = json!("late");
    store.write_batch(&mut writer, &[late]).unwrap();
    store.add_label("row-202", "q", "incorrect").unwrap();
    let checkpoint: (i64, i64, i64) = writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(checkpoint, (0, 0, 0), "An idle export must not pin the WAL");
    let (deleted_page, cursor, ceiling, more) = session.next_page().unwrap();
    assert!(deleted_page.is_empty());
    assert_eq!((cursor, ceiling, more), (200, through, true));
    let mut rest = String::new();
    loop {
        let (page, _, ceiling, more) = session.next_page().unwrap();
        assert_eq!(ceiling, through);
        rest.push_str(&page);
        if !more {
            break;
        }
    }
    let actual: Vec<Value> = rest
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(actual.len(), 104);
    assert_eq!(actual[0]["id"], "row-202");
    assert_eq!(actual[0]["labels"][0]["label"], "incorrect");
    assert_eq!(actual.last().unwrap()["id"], "row-305");
    assert!(
        actual
            .iter()
            .all(|record| record["id"] != "row-201" && record["id"] != "late")
    );
    assert!(session.selection.as_ref().unwrap().is_autocommit());
    assert_eq!(
        writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn unfiltered_and_time_only_exports_keep_direct_bounded_pages() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("direct-export.sqlite"), 7, 1000).unwrap();
    let base = captured_record("fixture", 0, 0, "model");
    let now = base["timestamp"].as_i64().unwrap();
    let records: Vec<_> = (0..205)
        .map(|index| {
            let mut record = base.clone();
            record["id"] = json!(format!("direct-{index:03}"));
            record["timestamp"] = json!(now - index);
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    for filter in [
        all_history(),
        Filter {
            from: Some(now - 204),
            to: Some(now),
            source: Some(String::new()),
            model: Some(String::new()),
            ..all_history()
        },
    ] {
        let mut session = store.start_export(&filter, "jsonl").unwrap();
        assert!(session.selection.is_none());
        let (first, cursor, ceiling, more) = session.next_page().unwrap();
        assert_eq!((cursor, ceiling, more), (100, 205, true));
        assert_eq!(first.lines().count(), 100);
        assert_eq!(
            store.export(&filter, "jsonl").unwrap(),
            reference_export(&store, &filter, "jsonl")
        );
    }
}

#[test]
fn jsonl_defers_a_fitting_large_record_without_advancing_or_losing_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("large-export.sqlite"), 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let filter = Filter {
        model: Some("fixture".into()),
        ..all_history()
    };
    // The first record is below the 2 MiB target. The second fits 32 MiB
    // individually, but appending it to the first would exceed that hard cap.
    for (id, request, response) in [
        ("small", 1536 * 1024, 0),
        (
            "large",
            15 * 1024 * 1024 + 512 * 1024,
            15 * 1024 * 1024 + 512 * 1024,
        ),
        ("tail", 0, 0),
    ] {
        store
            .write_batch(
                &mut writer,
                &[captured_record(id, request, response, "fixture")],
            )
            .unwrap();
    }
    let mut session = store.start_export(&filter, "jsonl").unwrap();
    let (first, cursor, through, more) = session.next_page().unwrap();
    assert!(more);
    assert_eq!((cursor, through), (1, 3));
    assert!(first.len() < 2 * 1024 * 1024);
    assert_eq!(first.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["id"],
        "small"
    );

    let (second, next, ceiling, more) = session.next_page().unwrap();
    assert!(more);
    assert_eq!((next, ceiling), (2, through));
    assert!(second.len() <= 32 * 1024 * 1024);
    assert!(first.len() + second.len() > 32 * 1024 * 1024);
    assert_eq!(second.lines().count(), 1);
    let actual: Value = serde_json::from_str(&second).unwrap();
    assert_eq!(actual["id"], "large");
    assert_eq!(actual["answers"][0]["valid"], true);
    let saved = store.request("large").unwrap().unwrap();
    assert_eq!(
        actual, saved,
        "the deferred record must be complete and unchanged"
    );
    drop((actual, saved, second));

    let (last, next, ceiling, more) = session.next_page().unwrap();
    assert!(!more);
    assert_eq!((next, ceiling), (3, through));
    assert_eq!(last.lines().count(), 1);
    assert_eq!(serde_json::from_str::<Value>(&last).unwrap()["id"], "tail");

    let mut oversized = captured_record(
        "oversized",
        16 * 1024 * 1024 - 100,
        16 * 1024 * 1024 - 100,
        "fixture",
    );
    oversized["source"] = json!("oversized-only");
    store.write_batch(&mut writer, &[oversized]).unwrap();
    let filter = Filter {
        source: Some("oversized-only".into()),
        ..filter
    };
    let error = store
        .start_export(&filter, "jsonl")
        .unwrap()
        .next_page()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "An export record exceeds the 32 MiB page limit"
    );
}

#[test]
fn csv_defers_complete_rows_and_emits_one_header() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("csv-export.sqlite"), 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    for (id, model) in [
        ("small", "m".repeat(1024)),
        (
            "large",
            format!("=fixture,\"quoted\"\n{}", "m".repeat(31 * 1024)),
        ),
        ("tail", "fixture".into()),
    ] {
        store
            .write_batch(&mut writer, &[captured_record(id, 0, 0, &model)])
            .unwrap();
    }
    let filter = Filter {
        source: Some("s".repeat(256)),
        ..all_history()
    };
    let (expected, _, _, more) = store
        .start_export(&filter, "csv")
        .unwrap()
        .next_page()
        .unwrap();
    assert!(!more);
    assert!(expected.contains("\"'=fixture,\"\"quoted\"\"\n"));
    let header = expected.split_inclusive('\n').next().unwrap();
    let mut collected = String::new();
    let mut session = store.start_export(&filter, "csv").unwrap();
    for expected_cursor in 1..=3 {
        // Scaled limits exercise the same production paging path without
        // creating enormous indexed model/source fields in the test database.
        let (page, next, through, more) = session.next_page_with_limits(2048, 32 * 1024).unwrap();
        assert_eq!(next, expected_cursor);
        assert_eq!(through, 3);
        assert_eq!(more, expected_cursor != 3);
        assert_eq!(page.starts_with(header), expected_cursor == 1);
        assert!(
            page.len()
                <= 32 * 1024
                    + if expected_cursor == 1 {
                        header.len()
                    } else {
                        0
                    }
        );
        collected.push_str(&page);
    }
    assert_eq!(collected, expected);
}

#[test]
fn csv_first_row_at_the_data_cap_progresses_and_empty_pages_keep_one_header() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("csv-boundary.sqlite"), 7, 100).unwrap();
    let filter = Filter {
        source: Some("s".repeat(256)),
        ..all_history()
    };
    let (header, cursor, through, more) = store
        .start_export(&filter, "csv")
        .unwrap()
        .next_page_with_limits(1, 1)
        .unwrap();
    assert_eq!((cursor, through, more), (0, 0, false));
    assert!(header.starts_with("id,timestamp,"));
    assert_eq!(header.lines().count(), 1);
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[captured_record("only", 0, 0, "fixture")],
        )
        .unwrap();
    let expected = store
        .start_export(&filter, "csv")
        .unwrap()
        .next_page()
        .unwrap()
        .0;
    let row_bytes = expected.len() - header.len();
    let mut session = store.start_export(&filter, "csv").unwrap();
    let (page, cursor, through, more) = session.next_page_with_limits(1, row_bytes).unwrap();
    assert_eq!(page, expected);
    assert_eq!((cursor, through, more), (1, 1, false));
    assert_eq!(page.len(), row_bytes + header.len());
    assert!(
        store
            .start_export(&filter, "csv")
            .unwrap()
            .next_page_with_limits(1, row_bytes - 1)
            .is_err()
    );
    let (empty, cursor, through, more) = session.next_page_with_limits(1, row_bytes).unwrap();
    assert!(empty.is_empty());
    assert_eq!((cursor, through, more), (1, 1, false));
}
