use super::*;

#[test]
fn basic_credential_discovery_keeps_the_encoded_component_opaque() {
    let access = crate::access::Access::test();
    let header = access.basic_header();
    let component = header.strip_prefix("Basic ").unwrap();
    for value in [header.to_owned(), format!("bAsIc \t{component} ")] {
        let mut found = Vec::new();
        collect_secret_string(&value, &mut found);
        assert_eq!(found, [value, component.to_owned()]);
        assert!(!found.iter().any(|secret| secret.contains(access.token())));
    }
}

fn records() -> Vec<(&'static str, Value)> {
    vec![
        (
            "observer-jsonl",
            normalize(
                &Capture {
                    id: "synthetic-observer-event".into(),
                    source: "synthetic-source".into(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    status: 200,
                    capture_complete: true,
                    request: br#"{"questions":{}}"#.to_vec(),
                    response: br#"{"answers":{}}"#.to_vec(),
                    ..Default::default()
                },
                &NormalizeOptions::default(),
            ),
        ),
        (
            "systemone-capture",
            json!({
                "id":"synthetic-capture-event", "source":"synthetic-source", "provider":"typesafe",
                "timestamp":chrono::Utc::now().timestamp_millis(), "status":200, "capture_complete":true,
                "request":{"questions":{}}, "response":{"answers":{}}
            }),
        ),
        (
            "jevrouter-receipt",
            serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap(),
        ),
    ]
}

fn extra<'a>(format: &str, record: &'a mut Value) -> &'a mut Value {
    match format {
        "observer-jsonl" => &mut record["request_extra"],
        "systemone-capture" => &mut record["request"],
        _ => record,
    }
}

#[test]
fn basic_credential_import_headers_redact_full_and_encoded_echoes() {
    let access = crate::access::Access::test();
    let header = access.basic_header();
    let component = header.strip_prefix("Basic ").unwrap();
    for prefix in [
        "Basic ", "basic ", "BASIC ", "bAsIc ", "Basic  ", "bAsIc   ",
    ] {
        let header = format!("{prefix}{component}");
        let header = header.as_str();
        for (format, mut record) in records() {
            let extra = extra(format, &mut record);
            extra["headers"] = json!({"Authorization":header, "X-Observer-Access":access.token()});
            extra["echo"] = json!([header, component, access.token()]);
            extra["ordinary_extension"] = json!("ordinary retained text");
            let imported =
                import_records(&record.to_string(), format, &NormalizeOptions::default()).unwrap();
            let retained = imported[0].to_string();
            for secret in [header, component, access.token()] {
                assert!(
                    !retained.contains(secret),
                    "known Basic credential survived {format}"
                );
            }
            assert!(retained.contains("ordinary retained text"));
            assert!(retained.contains(REDACTED));
        }
    }
}

#[test]
fn basic_credential_import_does_not_guess_secrets_from_ordinary_strings() {
    let access = crate::access::Access::test();
    let header = access.basic_header();
    let component = header.strip_prefix("Basic ").unwrap();
    for (format, mut record) in records() {
        extra(format, &mut record)["ordinary_extension"] = json!([header, component]);
        let imported =
            import_records(&record.to_string(), format, &NormalizeOptions::default()).unwrap();
        let retained = imported[0].to_string();
        assert!(
            retained.contains(header),
            "ordinary string changed in {format}"
        );
        assert!(
            retained.contains(component),
            "ordinary string changed in {format}"
        );
    }
}
