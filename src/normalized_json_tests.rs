use super::*;
use crate::model::{Capture, NormalizeOptions, import_records, normalize};
use crate::store::{Filter, Store};
use serde_json::json;

fn nested(depth: usize, leaf: Value) -> Value {
    (0..depth).fold(leaf, |value, _| json!([value]))
}

fn capture(location: &str, depth: usize) -> Capture {
    let extension = nested(depth, json!("synthetic-depth-secret"));
    let mut request =
        json!({"questions":{"q":{"type":"noul", "instructions":"Synthetic fixture"}}});
    let mut response = json!({"answers":{"q":{"type":"noul", "noul":0.5}}, "usage":{}});
    match location {
        "definition" => request["questions"]["q"]["extension"] = extension,
        "raw_answer" => response["answers"]["q"]["extension"] = extension,
        "request_extra" => request["extension"] = extension,
        "response_extra" => response["extension"] = extension,
        "usage_extra" => response["usage"]["extension"] = extension,
        "state" => request["state"] = extension,
        _ => panic!("unknown fixture"),
    }
    Capture {
        id: format!("historical-{location}-{depth}").into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        source: "synthetic-depth".into(),
        status: 200,
        duration_ms: 1.0,
        request: serde_json::to_vec(&request).unwrap(),
        response: serde_json::to_vec(&response).unwrap(),
        capture_complete: true,
        secret: Some("synthetic-depth-secret".into()),
        ..Default::default()
    }
}

#[test]
fn bounded_parser_handles_exact_depth_and_preserves_json_validation() {
    for object in [false, true] {
        for depth in [0, 127, 128, MAX_DEPTH, MAX_DEPTH + 1] {
            let (open, close) = if object { ("{\"v\":", "}") } else { ("[", "]") };
            let text = format!("{}0{}", open.repeat(depth), close.repeat(depth));
            assert_eq!(from_slice(text.as_bytes()).is_ok(), depth <= MAX_DEPTH);
            if depth <= MAX_DEPTH {
                let value = from_slice(text.as_bytes()).unwrap();
                check_value(&value).unwrap();
                assert_eq!(serde_json::to_string(&value).unwrap(), text);
            }
        }
    }
    let quoted = json!({"escaped":"[{}]\\\"\\\\\"", "many":"[".repeat(10_000)});
    let mut value = quoted;
    for _ in 1..MAX_DEPTH {
        value = json!([value]);
    }
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(from_slice(&bytes).unwrap(), value);
    check_value(&value).unwrap();
    assert!(check_value(&json!([value])).is_err());
    for malformed in [
        "{",
        "[}",
        "[0,]",
        "{\"x\":0,}",
        "\"\\q\"",
        "\"unterminated",
        "0 0",
    ] {
        assert!(from_slice(malformed.as_bytes()).is_err(), "{malformed}");
    }
    let deep = format!("{}0{} trailing", "[".repeat(128), "]".repeat(128));
    assert!(from_slice(deep.as_bytes()).is_err());
    assert!(from_slice(b"\"\xff\"").is_err());
    check_value(&json!(vec![0; 20_000])).unwrap();
}

#[test]
fn stored_wrapper_depth_recovers_detail_exports_and_roundtrip_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("historical-depth.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let options = NormalizeOptions {
        capture_state: true,
        ..Default::default()
    };
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    for (location, depth) in [
        ("definition", 124),
        ("raw_answer", 124),
        ("request_extra", 126),
        ("response_extra", 126),
        ("usage_extra", 125),
        ("state", 126),
    ] {
        let capture = capture(location, depth);
        assert!(serde_json::from_slice::<Value>(&capture.request).is_ok());
        assert!(serde_json::from_slice::<Value>(&capture.response).is_ok());
        let normalized = normalize(&capture, &options);
        assert_eq!(normalized["answers"][0]["valid"], true);
        assert!(!normalized.to_string().contains("synthetic-depth-secret"));
        // The normalizer's raw path is unchanged. These are the same serialized
        // rows older releases accepted; reopen without rewriting their content.
        store
            .write_batch(
                &mut store.writer_connection().unwrap(),
                std::slice::from_ref(&normalized),
            )
            .unwrap();
        let reopened = Store::open(&path, 7, 100).unwrap();
        let detail = reopened.request(&capture.id).unwrap().unwrap();
        assert_eq!(detail["answers"], normalized["answers"]);
        let filter = Filter {
            search: Some(capture.id.as_str().to_owned()),
            ..filter.clone()
        };
        let exported = reopened
            .start_export(&filter, "jsonl")
            .unwrap()
            .next_page()
            .unwrap()
            .0;
        assert_eq!(from_slice(exported.as_bytes()).unwrap(), detail);
        assert!(
            !reopened
                .start_export(&filter, "csv")
                .unwrap()
                .next_page()
                .unwrap()
                .0
                .is_empty()
        );
        let group = reopened
            .group(
                normalized["answers"][0]["group_id"].as_str().unwrap(),
                &filter,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            group["group"]["definition"],
            normalized["answers"][0]["definition"]
        );
        let mut expected = normalized;
        expected["source_event_id"] = expected["id"].clone();
        expected["import_format"] = json!("observer-jsonl");
        let first = import_records(&exported, "observer-jsonl", &options).unwrap();
        assert_eq!(first, vec![expected]);
        let again = import_records(&first[0].to_string(), "observer-jsonl", &options).unwrap();
        assert_eq!(again, first);
    }
}

#[test]
fn receipt_provenance_wrappers_remain_readable_and_roundtrip_exactly() {
    let mut receipt: Value =
        serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap();
    receipt["raw_jev"]["extension"] = nested(125, json!("synthetic-depth-secret"));
    receipt["raw_jev"]["usage"] = json!({});
    receipt["authorization"] = json!("Bearer synthetic-depth-secret");
    let text = receipt.to_string();
    assert!(serde_json::from_str::<Value>(&text).is_ok());
    let options = NormalizeOptions::default();
    let mut normalized = import_records(&text, "jevrouter-receipt", &options)
        .unwrap()
        .remove(0);
    assert!(!normalized.to_string().contains("synthetic-depth-secret"));
    assert!(serde_json::from_str::<Value>(&normalized.to_string()).is_err());
    check_value(&normalized).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("receipt-depth.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[normalized.clone()],
        )
        .unwrap();
    let reopened = Store::open(&path, 7, 100).unwrap();
    let detail = reopened
        .request(normalized["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(detail["receipt"], normalized["receipt"]);
    assert_eq!(detail["actions"], normalized["actions"]);
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let exported = reopened
        .start_export(&filter, "jsonl")
        .unwrap()
        .next_page()
        .unwrap()
        .0;
    assert_eq!(from_slice(exported.as_bytes()).unwrap(), detail);
    normalized["import_format"] = json!("observer-jsonl");
    let imported = import_records(&exported, "observer-jsonl", &options).unwrap();
    assert_eq!(imported, vec![normalized]);
    assert_eq!(
        import_records(&imported[0].to_string(), "observer-jsonl", &options).unwrap(),
        imported
    );
}

fn bounded_stack_work() {
    let options = NormalizeOptions {
        capture_state: true,
        ..Default::default()
    };
    let base = normalize(&capture("request_extra", 126), &options);
    for slot in ["request_extra", "definition", "raw_answer"] {
        let mut record = base.clone();
        match slot {
            "request_extra" => {
                record[slot] = json!({"authorization":"Bearer synthetic-depth-secret", "nested":nested(MAX_DEPTH - 2, json!("synthetic-depth-secret"))})
            }
            "definition" => {
                record["answers"][0][slot]["nested"] =
                    nested(MAX_DEPTH - 4, json!("synthetic-depth-secret"))
            }
            "raw_answer" => {
                record["answers"][0][slot]["nested"] =
                    nested(MAX_DEPTH - 4, json!("synthetic-depth-secret"))
            }
            _ => unreachable!(),
        }
        record["authorization"] = json!("Bearer synthetic-depth-secret");
        let bytes = serde_json::to_vec(&record).unwrap();
        let parsed = from_slice(&bytes).unwrap();
        check_value(&parsed).unwrap();
        let imported = import_records(
            std::str::from_utf8(&bytes).unwrap(),
            "observer-jsonl",
            &options,
        )
        .unwrap();
        assert_eq!(imported[0]["answers"][0]["valid"], true);
        assert!(!imported[0].to_string().contains("synthetic-depth-secret"));
        assert_eq!(
            import_records(&imported[0].to_string(), "observer-jsonl", &options).unwrap(),
            imported
        );
        drop((parsed, imported, record));
    }
    // This adversarial source is rejected before extended recursive parsing;
    // no deeply nested Value is ever constructed or dropped.
    let excessive = format!("{}0{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(from_slice(excessive.as_bytes()).is_err());
    // The raw-body parser and raw-capture import envelope still use their
    // original limit even though normalized Observer JSON has an allowance.
    let excessive_raw = capture("request_extra", 127);
    assert!(serde_json::from_slice::<Value>(&excessive_raw.request).is_err());
    let incomplete = normalize(&excessive_raw, &options);
    assert_eq!(
        incomplete["normalization_error"],
        "Request body is not a complete JSON object"
    );
    assert_eq!(incomplete["answers"][0]["valid"], false);
    // Transport completion remains separate from JSON parsing completeness.
    assert_eq!(incomplete["capture_complete"], true);
    let too_deep_raw = json!({"request":{"questions":{}, "nested":nested(126, json!(0))}});
    assert!(import_records(&too_deep_raw.to_string(), "systemone-capture", &options).is_err());
}

#[test]
fn parsing_redaction_normalization_and_drop_fit_a_bounded_stack() {
    const CHILD: &str = "JEV_OBSERVER_JSON_DEPTH_CHILD";
    if std::env::var_os(CHILD).is_some() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(bounded_stack_work)
            .unwrap()
            .join()
            .unwrap();
        return;
    }
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "normalized_json::tests::parsing_redaction_normalization_and_drop_fit_a_bounded_stack",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "Bounded-stack child failed: {status}");
            break;
        }
        if started.elapsed() > Duration::from_secs(5) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Bounded-stack child exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
