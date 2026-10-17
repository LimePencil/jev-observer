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

#[test]
fn jsonl_defers_a_fitting_large_record_without_advancing_or_losing_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("large-export.sqlite"), 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let filter = all_history();
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
    let (first, cursor, through, more) = store.export_page(&filter, "jsonl", 0, None).unwrap();
    assert!(more);
    assert_eq!((cursor, through), (1, 3));
    assert!(first.len() < 2 * 1024 * 1024);
    assert_eq!(first.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["id"],
        "small"
    );

    let (second, next, ceiling, more) = store
        .export_page(&filter, "jsonl", cursor, Some(through))
        .unwrap();
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

    let (last, next, ceiling, more) = store
        .export_page(&filter, "jsonl", next, Some(through))
        .unwrap();
    assert!(!more);
    assert_eq!((next, ceiling), (3, through));
    assert_eq!(last.lines().count(), 1);
    assert_eq!(serde_json::from_str::<Value>(&last).unwrap()["id"], "tail");

    let oversized = captured_record(
        "oversized",
        16 * 1024 * 1024 - 100,
        16 * 1024 * 1024 - 100,
        "fixture",
    );
    store.write_batch(&mut writer, &[oversized]).unwrap();
    let error = store.export_page(&filter, "jsonl", next, None).unwrap_err();
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
    let filter = all_history();
    let (expected, _, _, more) = store.export_page(&filter, "csv", 0, None).unwrap();
    assert!(!more);
    assert!(expected.contains("\"'=fixture,\"\"quoted\"\"\n"));
    let header = expected.split_inclusive('\n').next().unwrap();
    let mut collected = String::new();
    let mut after = 0;
    let mut ceiling = None;
    for expected_cursor in 1..=3 {
        // Scaled limits exercise the same production paging path without
        // creating enormous indexed model/source fields in the test database.
        let (page, next, through, more) = store
            .export_page_with_limits(&filter, "csv", after, ceiling, 2048, 32 * 1024)
            .unwrap();
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
        after = next;
        ceiling = Some(through);
    }
    assert_eq!(collected, expected);
}

#[test]
fn csv_first_row_at_the_data_cap_progresses_and_empty_pages_keep_one_header() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("csv-boundary.sqlite"), 7, 100).unwrap();
    let filter = all_history();
    let (header, cursor, through, more) = store
        .export_page_with_limits(&filter, "csv", 0, None, 1, 1)
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
    let expected = store.export_page(&filter, "csv", 0, None).unwrap().0;
    let row_bytes = expected.len() - header.len();
    let (page, cursor, through, more) = store
        .export_page_with_limits(&filter, "csv", 0, None, 1, row_bytes)
        .unwrap();
    assert_eq!(page, expected);
    assert_eq!((cursor, through, more), (1, 1, false));
    assert_eq!(page.len(), row_bytes + header.len());
    assert!(
        store
            .export_page_with_limits(&filter, "csv", 0, None, 1, row_bytes - 1)
            .is_err()
    );
    let (empty, cursor, through, more) = store
        .export_page_with_limits(&filter, "csv", cursor, Some(through), 1, row_bytes)
        .unwrap();
    assert!(empty.is_empty());
    assert_eq!((cursor, through, more), (1, 1, false));
}
