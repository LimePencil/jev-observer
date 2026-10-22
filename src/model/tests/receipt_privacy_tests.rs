use super::*;
use crate::store::{Filter, Store};

const INPUT_FIELDS: [&str; 5] = ["state", "images", "image", "audio", "video"];

fn receipt_with_inputs() -> Value {
    let mut receipt: Value = serde_json::from_str(include_str!(
        "../../../fixtures/model/jevrouter-receipt.json"
    ))
    .unwrap();
    for field in INPUT_FIELDS {
        receipt["raw_jev"][field] = json!(format!("private-envelope-{field}"));
    }
    receipt["raw_jev"]["original_metadata"] = json!({});
    for field in INPUT_FIELDS {
        receipt["raw_jev"]["original_metadata"][field] =
            json!(format!("private-envelope-legacy-{field}"));
    }
    // The same names inside answer semantics are unrelated to input snapshots.
    receipt["raw_jev"]["answers"]["tool"]["extension"] = json!({
        "state":"approved", "images":["image category"], "image":"image label",
        "audio":"audio label", "video":"video label",
        "original_metadata":{"state":"answer meaning"}
    });
    receipt["raw_jev_stages"] = json!([{"response":receipt["raw_jev"], "stage":"single"}]);
    receipt
}

fn check_inputs(record: &Value, retained: bool) {
    let original = receipt_with_inputs();
    for provenance in [&record["receipt"], &record["actions"][0]] {
        for response in [
            &provenance["raw_jev"],
            &provenance["raw_jev_stages"][0]["response"],
        ] {
            for field in INPUT_FIELDS {
                if retained {
                    assert_eq!(response[field], original["raw_jev"][field]);
                    assert_eq!(
                        response["original_metadata"][field],
                        original["raw_jev"]["original_metadata"][field]
                    );
                } else {
                    assert!(response[field].is_null(), "Retained response input {field}");
                    assert!(
                        response["original_metadata"][field].is_null(),
                        "Retained legacy response input {field}"
                    );
                }
            }
            assert_eq!(response["answers"], original["raw_jev"]["answers"]);
        }
    }
    assert_eq!(record.to_string().contains("private-envelope-"), retained);
    assert_eq!(record["event_kind"], "application_action");
    assert!(record["answers"].as_array().unwrap().is_empty());
}

#[test]
fn receipt_provenance_inputs_require_capture_opt_in() {
    for capture_state in [false, true] {
        let records = import_records(
            &receipt_with_inputs().to_string(),
            "jevrouter-receipt",
            &NormalizeOptions {
                capture_state,
                ..Default::default()
            },
        )
        .unwrap();
        check_inputs(&records[0], capture_state);
    }
}

#[test]
fn receipt_input_preference_survives_storage_export_and_reimport() {
    let directory = tempfile::tempdir().unwrap();
    let original_store = Store::open(&directory.path().join("retained.sqlite"), 7, 100).unwrap();
    let records = import_records(
        &receipt_with_inputs().to_string(),
        "jevrouter-receipt",
        &NormalizeOptions {
            capture_state: true,
            ..Default::default()
        },
    )
    .unwrap();
    original_store
        .write_batch(&mut original_store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let exported = original_store.export(&filter, "jsonl").unwrap();
    assert!(exported.contains("private-envelope-"));

    for capture_state in [false, true] {
        let options = NormalizeOptions {
            capture_state,
            ..Default::default()
        };
        let records = import_records(&exported, "observer-jsonl", &options).unwrap();
        check_inputs(&records[0], capture_state);
        let store = Store::open(
            &directory
                .path()
                .join(format!("reimport-{capture_state}.sqlite")),
            7,
            100,
        )
        .unwrap();
        store
            .write_batch(&mut store.writer_connection().unwrap(), &records)
            .unwrap();
        let saved = store
            .request(records[0]["id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        check_inputs(&saved, capture_state);
        let reexported = store.export(&filter, "jsonl").unwrap();
        assert_eq!(reexported.contains("private-envelope-"), capture_state);
        let roundtrip = import_records(&reexported, "observer-jsonl", &options).unwrap();
        check_inputs(&roundtrip[0], capture_state);
    }
}
