use super::*;
use crate::store::{Filter, Store};

fn collision_capture(secret: &str, sequence: usize) -> Capture {
    Capture {
        id: format!("event-{sequence}-{secret}"),
        timestamp: chrono::Utc::now().timestamp_millis(),
        source: format!("fixture-{secret}"),
        task_version: Some(format!("version-{secret}")),
        status: 200,
        duration_ms: 1.5,
        capture_complete: true,
        transport_error: Some(format!("detail-{secret}")),
        secret: Some(secret.into()),
        request: serde_json::to_vec(&json!({
            "questions":{"q":{"type":"noul", "instructions":format!("Check {secret}")}},
            "credentials":secret,
            "extension":{(secret):format!("echo-{secret}")},
            "headers":{"private":"discarded-header"}
        }))
        .unwrap(),
        response: serde_json::to_vec(&json!({
            "answers":{"q":{"type":"noul", "noul":0.8}},
            "extension":{(secret):format!("echo-{secret}")}
        }))
        .unwrap(),
        ..Default::default()
    }
}

#[test]
fn generated_envelope_fields_survive_secret_name_collisions() {
    for (index, secret) in [
        "request",
        "response",
        "id",
        "source",
        "task_version",
        "transport_error",
    ]
    .into_iter()
    .enumerate()
    {
        let capture = collision_capture(secret, index);
        let original_request = capture.request.clone();
        let original_response = capture.response.clone();
        let record = normalize(&capture, &NormalizeOptions::default());
        assert_eq!(
            record["id"],
            format!("event-{index}-{REDACTED}"),
            "{secret}"
        );
        assert_eq!(record["source"], format!("fixture-{REDACTED}"), "{secret}");
        assert_eq!(
            record["transport_error"],
            format!("detail-{REDACTED}"),
            "{secret}"
        );
        assert!(
            record["normalization_error"].is_null(),
            "{secret}: {record}"
        );
        assert_eq!(record["answers"].as_array().unwrap().len(), 1, "{secret}");
        assert_eq!(
            record["answers"][0]["task_version"],
            format!("version-{REDACTED}")
        );
        assert_eq!(
            record["answers"][0]["definition"]["instructions"],
            format!("Check {REDACTED}")
        );
        assert_eq!(record["answers"][0]["definition_redacted"], true);
        assert_eq!(record["answers"][0]["valid"], false);
        assert_eq!(record["request_extra"]["credentials"], REDACTED);
        assert!(record["request_extra"].get("headers").is_none());
        for field in ["request_extra", "response_extra"] {
            assert_eq!(
                record[field]["extension"][REDACTED],
                format!("echo-{REDACTED}")
            );
            assert!(record[field]["extension"].get(secret).is_none());
        }
        let other = normalize(
            &collision_capture(secret, index + 100),
            &NormalizeOptions::default(),
        );
        assert_ne!(
            record["answers"][0]["group_id"],
            other["answers"][0]["group_id"]
        );
        assert_eq!(capture.request, original_request);
        assert_eq!(capture.response, original_response);
    }
}

#[test]
fn generated_envelope_values_still_obey_configured_private_fields() {
    let capture = Capture {
        id: "event-1".into(),
        source: "private-context".into(),
        status: 200,
        capture_complete: true,
        request: br#"{"questions":{},"extension":"private-context"}"#.to_vec(),
        response: br#"{"answers":{}}"#.to_vec(),
        ..Default::default()
    };
    let options = NormalizeOptions {
        redact_keys: vec!["source".into()],
        ..Default::default()
    };
    let record = normalize(&capture, &options);
    assert_eq!(record["id"], "event-1");
    assert_eq!(record["source"], REDACTED);
    assert_eq!(record["request_extra"]["extension"], REDACTED);
}

#[test]
fn raw_imports_with_envelope_collisions_persist_and_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("envelopes.sqlite"), 7, 100).unwrap();
    for (index, secret) in [
        "i",
        "id",
        "request",
        "response",
        "source",
        "task_version",
        "transport_error",
    ]
    .into_iter()
    .enumerate()
    {
        let raw = json!({
            "id":format!("import-{index}"), "source":"fixture", "provider":"typesafe",
            "timestamp":chrono::Utc::now().timestamp_millis(), "status":200, "capture_complete":true,
            "request":{"questions":{}, "credentials":secret, "echo":format!("echo-{secret}")},
            "response":{"answers":{}}
        });
        let records = import_records(
            &raw.to_string(),
            "systemone-capture",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert!(
            records[0]["id"].as_str().is_some_and(|id| !id.is_empty()),
            "{secret}"
        );
        assert_eq!(
            records[0]["request_extra"]["echo"],
            format!("echo-{REDACTED}"),
            "{secret}"
        );
        assert_eq!(
            store
                .write_batch(&mut store.writer_connection().unwrap(), &records)
                .unwrap(),
            1,
            "{secret}"
        );
    }
    let exported = store.export(&Filter::default(), "jsonl").unwrap();
    let reimported =
        import_records(&exported, "observer-jsonl", &NormalizeOptions::default()).unwrap();
    assert_eq!(reimported.len(), 7);
    let restored = Store::open(&directory.path().join("restored.sqlite"), 7, 100).unwrap();
    assert_eq!(
        restored
            .write_batch(&mut restored.writer_connection().unwrap(), &reimported)
            .unwrap(),
        7
    );
    for record in reimported {
        let saved = restored
            .request(record["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(saved["request_extra"], record["request_extra"]);
        assert_eq!(saved["source"], record["source"]);
    }
}
