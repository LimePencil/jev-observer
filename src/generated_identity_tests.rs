use super::*;
use crate::store::{Filter, Store};

fn live_capture() -> Capture {
    Capture {
        id: CaptureId::new_live(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        source: "fixture".into(),
        status: 200,
        capture_complete: true,
        request: br#"{"questions":{"q":{"type":"noul","instructions":"7"}},"password":"7"}"#
            .to_vec(),
        response: br#"{"answers":{"q":{"type":"noul","noul":0.5}}}"#.to_vec(),
        ..Default::default()
    }
}

#[test]
fn generated_live_identity_survives_privacy_before_definition_isolation() {
    let first = live_capture();
    let second = live_capture();
    for options in [
        NormalizeOptions::default(),
        NormalizeOptions {
            redact_keys: vec!["id".into()],
            ..Default::default()
        },
    ] {
        let one = normalize(&first, &options);
        let two = normalize(&second, &options);
        assert_eq!(one["id"], first.id.as_str());
        assert_eq!(two["id"], second.id.as_str());
        assert_ne!(one["id"], two["id"]);
        assert_eq!(one["answers"][0]["definition"]["instructions"], REDACTED);
        assert_eq!(one["answers"][0]["definition_redacted"], true);
        assert_ne!(one["answers"][0]["group_id"], two["answers"][0]["group_id"]);
        let answer = &one["answers"][0];
        assert_eq!(
            answer["group_id"],
            group_fingerprint(
                &one["source"],
                &answer["key"],
                &answer["definition_id"],
                &answer["presentation_id"],
                &answer["task_version"],
                &json!(first.id),
            )
        );
    }
}

#[test]
fn caller_supplied_uuid_and_fingerprint_strings_remain_private_data() {
    for supplied in [
        CaptureId::new_live().as_str().to_owned(),
        CaptureId::raw_import("source", "typesafe", "event")
            .as_str()
            .to_owned(),
    ] {
        let mut capture = live_capture();
        capture.id = supplied.clone().into();
        capture.secret = Some(supplied);
        assert_eq!(
            normalize(&capture, &NormalizeOptions::default())["id"],
            REDACTED
        );
    }
}

#[test]
fn raw_generated_identity_keeps_distinct_calls_duplicates_and_export_roundtrips() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("generated-import.sqlite"), 7, 100).unwrap();
    let options = NormalizeOptions::default();
    let credentials: Vec<_> = "0123456789abcdef"
        .chars()
        .map(|value| value.to_string())
        .collect();
    let raw = |id: &str, credentials: Value| {
        json!({
            "id":id, "source":"fixture", "provider":"typesafe",
            "timestamp":chrono::Utc::now().timestamp_millis(), "status":200,
            "capture_complete":true,
            "request":{"questions":{}, "credentials":credentials},
            "response":{"answers":{}}
        })
    };
    let text = [
        raw("one", json!(credentials)),
        raw("two", json!(credentials)),
        raw("one", Value::Null),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let records = import_records(&text, "systemone-capture", &options).unwrap();
    assert_ne!(records[0]["id"], records[1]["id"]);
    assert_eq!(
        records[0]["id"], records[2]["id"],
        "A different secret set cannot change explicit event identity"
    );
    for (record, id) in records.iter().zip(["one", "two", "one"]) {
        assert_eq!(
            record["id"],
            CaptureId::raw_import("fixture", "typesafe", id).as_str()
        );
        assert_eq!(record["source_event_id"], record["id"]);
    }
    let mut writer = store.writer_connection().unwrap();
    assert_eq!(store.write_batch(&mut writer, &records).unwrap(), 2);
    assert_eq!(store.write_batch(&mut writer, &records).unwrap(), 0);
    let exported = store
        .export(
            &Filter {
                window: Some("all".into()),
                ..Default::default()
            },
            "jsonl",
        )
        .unwrap();
    let again = import_records(&exported, "observer-jsonl", &options).unwrap();
    assert_eq!(again.len(), 2);
    for (record, original) in again.iter().zip(&records) {
        assert_eq!(record["id"], original["id"]);
        assert_eq!(record["source_event_id"], original["source_event_id"]);
    }
    assert_eq!(store.write_batch(&mut writer, &again).unwrap(), 0);
}
