use super::*;
use crate::model::{Capture, NormalizeOptions, import_records, normalize};

fn record(id: &str) -> Value {
    normalize(&Capture {
        id:id.into(), timestamp:Utc::now().timestamp_millis(), source:"review-test".into(),
        status:200, capture_complete:true,
        request: serde_json::to_vec(&json!({"model":"test", "questions":{"urgent":{"type":"noul", "instructions":"Urgent?"}}})).unwrap(),
        response: serde_json::to_vec(&json!({"answers":{"urgent":{"type":"noul","noul":0.8,"confidence":0.9}}})).unwrap(),
        ..Default::default()
    }, &NormalizeOptions::default())
}

fn annotation(source: &str, timestamp: i64) -> Value {
    json!({"key":"urgent", "label":"correct", "timestamp":timestamp,
        "source":source, "source_event_id":"review-123", "reason":"Supplied review rationale",
        "reviewer":{"name":"Synthetic reviewer", "rubric":["a","b"]}})
}

#[test]
fn imported_review_metadata_survives_details_export_and_reimport() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source.sqlite"), 7, 100).unwrap();
    let mut original = record("reviewed");
    let expected = annotation("independent-reviewer", 1_700_000_000_000);
    original["labels"] = json!([expected]);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[original])
        .unwrap();
    assert_eq!(
        store.request("reviewed").unwrap().unwrap()["labels"],
        json!([expected])
    );
    let exported = store.export(&Filter::default(), "jsonl").unwrap();
    let parsed: Value = serde_json::from_str(exported.trim()).unwrap();
    assert_eq!(parsed["labels"], json!([expected]));
    let reimported =
        import_records(&exported, "observer-jsonl", &NormalizeOptions::default()).unwrap();
    let restored = Store::open(&dir.path().join("restored.sqlite"), 7, 100).unwrap();
    restored
        .write_batch(&mut restored.writer_connection().unwrap(), &reimported)
        .unwrap();
    assert_eq!(
        restored.request("reviewed").unwrap().unwrap()["labels"],
        json!([expected])
    );
}

#[test]
fn local_review_replacement_clears_imported_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("reviews.sqlite"), 7, 100).unwrap();
    let mut original = record("reviewed");
    // Imported annotations may themselves have come from another Observer.
    let expected = annotation("local-reviewer", Utc::now().timestamp_millis());
    original["labels"] = json!([expected]);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[original])
        .unwrap();
    assert_eq!(
        store.request("reviewed").unwrap().unwrap()["labels"],
        json!([expected])
    );
    assert_eq!(
        store.add_label("reviewed", "urgent", "correct").unwrap(),
        LabelOutcome::Saved
    );
    // Exercise the exact same timestamp/value case rather than relying on clock timing.
    store
        .writer_connection()
        .unwrap()
        .execute(
            "UPDATE labels SET timestamp=? WHERE request_id='reviewed'",
            [expected["timestamp"].as_i64().unwrap()],
        )
        .unwrap();
    let reviewed = store.request("reviewed").unwrap().unwrap();
    assert_eq!(
        reviewed["labels"],
        json!([{
            "key":"urgent", "label":"correct", "timestamp":expected["timestamp"], "source":"local-reviewer"
        }])
    );
    let exported = store.export(&Filter::default(), "jsonl").unwrap();
    assert!(!exported.contains("review-123"));
    assert!(!exported.contains("Supplied review rationale"));
}

#[test]
fn legacy_review_metadata_recovers_only_the_last_matching_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let expected = annotation("independent-reviewer", 1_700_000_000_000);
    let mut matching = record("matching");
    let mut first = expected.clone();
    first["source_event_id"] = json!("earlier-review");
    matching["labels"] = json!([first, expected]);
    let mut changed = record("changed");
    let mut latest = expected.clone();
    latest["label"] = json!("incorrect");
    changed["labels"] = json!([expected, latest]);
    let mut missing_timestamp = record("missing-timestamp");
    let mut unstamped = expected.clone();
    unstamped.as_object_mut().unwrap().remove("timestamp");
    missing_timestamp["labels"] = json!([unstamped]);
    let mut local = record("local");
    local["labels"] = json!([annotation("local-reviewer", 1_700_000_000_000)]);
    let mut writer = store.writer_connection().unwrap();
    store
        .write_batch(&mut writer, &[matching, changed, missing_timestamp, local])
        .unwrap();
    writer
        .execute(
            "UPDATE labels SET label='correct' WHERE request_id='changed'",
            [],
        )
        .unwrap();
    // Recreate the v0.3.1 schema, then exercise the actual open-time upgrade.
    writer
        .execute_batch("ALTER TABLE labels DROP COLUMN annotation")
        .unwrap();
    drop(writer);
    drop(store);
    let reopened = Store::open(&path, 7, 100).unwrap();
    assert_eq!(
        reopened.request("matching").unwrap().unwrap()["labels"],
        json!([expected])
    );
    for id in ["changed", "missing-timestamp", "local"] {
        let label = reopened.request(id).unwrap().unwrap()["labels"][0].clone();
        assert!(
            label.get("source_event_id").is_none(),
            "Stale metadata recovered for {id}"
        );
        assert!(
            label.get("reason").is_none(),
            "Stale metadata recovered for {id}"
        );
    }
    // The old binary's explicit-column local write stays compatible and clears notes.
    reopened.writer_connection().unwrap().execute(
        "INSERT OR REPLACE INTO labels(request_id,key,label,timestamp,source) VALUES('matching','urgent','correct',?,'local-reviewer')",
        [expected["timestamp"].as_i64().unwrap()],
    ).unwrap();
    assert!(
        reopened.request("matching").unwrap().unwrap()["labels"][0]
            .get("reason")
            .is_none()
    );
    let again = Store::open(&path, 7, 100).unwrap();
    assert_eq!(
        again.request("matching").unwrap(),
        reopened.request("matching").unwrap()
    );
}

#[test]
fn imported_review_defaults_and_duplicate_keys_preserve_current_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("defaults.sqlite"), 7, 100).unwrap();
    let mut original = record("reviewed");
    original["labels"] = json!([
        annotation("independent-reviewer", 1_700_000_000_000),
        {"question_key":"urgent", "label":"unknown", "reason":"New assessment"}
    ]);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[original])
        .unwrap();
    let labels = store.request("reviewed").unwrap().unwrap()["labels"].clone();
    assert_eq!(labels.as_array().unwrap().len(), 1);
    assert_eq!(labels[0]["key"], "urgent");
    assert_eq!(labels[0]["label"], "unknown");
    assert_eq!(labels[0]["source"], "import");
    assert!(labels[0]["timestamp"].is_i64());
    assert_eq!(labels[0]["reason"], "New assessment");
    assert!(labels[0].get("source_event_id").is_none());
}
