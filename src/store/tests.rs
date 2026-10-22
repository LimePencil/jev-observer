use super::*;

#[test]
fn borrowed_storage_json_keeps_field_order_and_nested_values() {
    let original = r#"{"first":1,"middle":{"z":["한글",null],"a":true},"last":"tail"}"#;
    let value: Value = serde_json::from_str(original).unwrap();
    let fields = value.as_object().unwrap();
    for (excluded, expected) in [
        (
            "first",
            r#"{"last":"tail","middle":{"z":["한글",null],"a":true}}"#,
        ),
        ("middle", r#"{"first":1,"last":"tail"}"#),
        (
            "last",
            r#"{"first":1,"middle":{"z":["한글",null],"a":true}}"#,
        ),
        ("absent", original),
    ] {
        assert_eq!(serialize_without_field(fields, excluded).unwrap(), expected);
        assert_eq!(serde_json::to_string(&value).unwrap(), original);
    }
    assert_eq!(
        serialize_without_field(&serde_json::Map::new(), "missing").unwrap(),
        "{}"
    );
    assert_eq!(
        serialize_without_field(json!({"only": [1, 2]}).as_object().unwrap(), "only").unwrap(),
        "{}"
    );
}
use sha2::Digest;

pub(super) fn assert_overview_matches_detail(overview: &Value, detail: &Value) {
    let mut shared = detail.clone();
    for field in [
        "warning_count",
        "review_counts",
        "mean_latency_ms",
        "cost_usd",
        "cost_known_requests",
        "input_tokens",
        "output_tokens",
        "input_tokens_known_requests",
        "output_tokens_known_requests",
        "input_tokens_overflow",
        "output_tokens_overflow",
        "error_count",
    ] {
        assert!(overview.get(field).is_none(), "detail-only field {field}");
        assert!(shared.as_object_mut().unwrap().remove(field).is_some());
    }
    assert_eq!(*overview, shared);
}

#[test]
fn encrypted_history_rejects_wrong_key_and_migrates_existing_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let plain = Store::open(&path, 7, 100).unwrap();
    plain
        .write_batch(&mut plain.writer_connection().unwrap(), &[record("before")])
        .unwrap();
    plain.set_credential_approval(Some("digest-only")).unwrap();
    drop(plain);
    let key = "a".repeat(64);
    let encrypted = Store::open_encrypted(&path, key.clone(), 7, 100).unwrap();
    assert!(!plaintext_database(&path).unwrap());
    assert_eq!(
        encrypted.credential_approval().unwrap().as_deref(),
        Some("digest-only")
    );
    let without_key = Connection::open(&path).unwrap();
    assert!(
        without_key
            .query_row("SELECT count(*) FROM requests", [], |row| row
                .get::<_, i64>(0))
            .is_err()
    );
    assert_eq!(
        encrypted
            .export(
                &Filter {
                    window: Some("all".into()),
                    ..Filter::default()
                },
                "jsonl"
            )
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(Store::open_encrypted(&path, "b".repeat(64), 7, 100).is_err());
    let reopened = Store::open_encrypted(&path, key, 7, 100).unwrap();
    assert_eq!(
        reopened
            .export(
                &Filter {
                    window: Some("all".into()),
                    ..Filter::default()
                },
                "jsonl"
            )
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(Store::open_encrypted(&path, "short".into(), 7, 100).is_err());
}

#[cfg(unix)]
#[test]
fn named_pipe_database_is_rejected_before_io_or_permission_changes() {
    const CHILD_PATH: &str = "JEV_OBSERVER_DATABASE_FIFO_TEST_PATH";
    const CHILD_MODE: &str = "JEV_OBSERVER_DATABASE_FIFO_TEST_MODE";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let result = if std::env::var(CHILD_MODE).unwrap() == "live" {
            Store::open_encrypted(Path::new(&path), "c".repeat(64), 7, 100)
        } else {
            Store::open_demo(Path::new(&path), 7, 100)
        };
        let error = result.err().expect("Named-pipe database was accepted");
        assert!(error.to_string().contains("regular file"), "{error:#}");
        return;
    }
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        },
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    for mode in ["live", "demo"] {
        for linked in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("history.pipe");
            let name = CString::new(target.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
            let path = if linked {
                let link = directory.path().join("history.sqlite");
                symlink(&target, &link).unwrap();
                link
            } else {
                target.clone()
            };
            // A blocking open must fail this test rather than strand a test
            // thread. No writer is ever attached to either FIFO path.
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::tests::named_pipe_database_is_rejected_before_io_or_permission_changes",
                ])
                .env(CHILD_PATH, &path)
                .env(CHILD_MODE, mode)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let started = Instant::now();
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert_eq!(
                        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                        0o640,
                        "{mode} changed named-pipe permissions (linked={linked})"
                    );
                    assert!(
                        status.success(),
                        "{mode} accepted a non-file (linked={linked})"
                    );
                    if linked {
                        assert!(path.is_symlink());
                    }
                    break;
                }
                if started.elapsed() > Duration::from_secs(2) {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{mode} named-pipe database blocked startup (linked={linked})");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn plaintext_migration_follows_database_symlink_without_leaving_target_plaintext() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target.sqlite");
    let link = directory.path().join("history.sqlite");
    let plain = Store::open(&target, 7, 100).unwrap();
    plain
        .write_batch(
            &mut plain.writer_connection().unwrap(),
            &[record("before-migration")],
        )
        .unwrap();
    drop(plain);
    symlink(&target, &link).unwrap();
    let encrypted = Store::open_encrypted(&link, "c".repeat(64), 7, 100).unwrap();
    assert!(encrypted.request("before-migration").unwrap().is_some());
    drop(encrypted);
    assert!(!plaintext_database(&target).unwrap());
    let reopened = Store::open_encrypted(&link, "c".repeat(64), 7, 100).unwrap();
    assert!(reopened.request("before-migration").unwrap().is_some());
    assert!(link.is_symlink());
}

#[test]
fn system_credential_approval_is_revocable_without_storing_the_token() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("approval.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let token = "jo_local_dummy-secret-token";
    let hash = format!("{:x}", sha2::Sha256::digest(token.as_bytes()));
    store.set_credential_approval(Some(&hash)).unwrap();
    assert_eq!(
        store.credential_approval().unwrap().as_deref(),
        Some(hash.as_str())
    );
    assert!(
        !std::fs::read(&path)
            .unwrap()
            .windows(token.len())
            .any(|bytes| bytes == token.as_bytes())
    );
    store.set_credential_approval(None).unwrap();
    assert!(store.credential_approval().unwrap().is_none());
    let reopened = Store::open(&path, 7, 100).unwrap();
    assert!(reopened.credential_approval().unwrap().is_none());
}
fn record(id: &str) -> Value {
    json!({"schema_version":1,"id":id,"timestamp":Utc::now().timestamp_millis(),"source":"test","model":"jev-test","status":200,"duration_ms":4.0,"cost_usd":0.1,"cost_basis":"synthetic","input_tokens":100,"output_tokens":3,"capture_complete":true,"sample":true,"answers":[{"key":"urgent","kind":"noul","group_id":"g","definition_id":"d","presentation_id":"p","definition":{"type":"noul","instructions":"Urgent?"},"valid":true,"value":0.8}]})
}

fn dashboard_temp_storage_case(encrypted: bool) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("temporary-payload.sqlite");
    let store = if encrypted {
        Store::open_encrypted(&path, "d".repeat(64), 7, 100).unwrap()
    } else {
        Store::open_demo(&path, 7, 100).unwrap()
    };
    let mut saved = record("synthetic-large-definition");
    saved["answers"][0]["definition"]["instructions"] =
        json!("SYNTHETIC_DASHBOARD_TEMP_MARKER_Q7Z9 ".repeat(95_000));
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[saved.clone()])
        .unwrap();
    let base = Filter {
        window: Some("all".into()),
        as_of: saved["timestamp"].as_i64(),
        ..Default::default()
    };
    let expected = store.dashboard(&base).unwrap();
    let expected_detail = store.group("g", &base).unwrap().unwrap();
    assert_eq!(expected["groups"].as_array().unwrap().len(), 1);
    assert_eq!(expected["requests"].as_array().unwrap().len(), 1);
    assert_eq!(
        expected["groups"][0]["definition"],
        saved["answers"][0]["definition"]
    );
    for filter in [
        base.clone(),
        Filter {
            search: Some("test".into()),
            ..base.clone()
        },
        Filter {
            group: Some("g".into()),
            ..base.clone()
        },
        Filter {
            search: Some("test".into()),
            group: Some("g".into()),
            ..base.clone()
        },
    ] {
        let mut connection = store.reader().unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        // Search/group selection and subsequent full-definition metadata
        // sorting share one connection and must preserve its privacy mode.
        let dashboard = Store::dashboard_from(&mut connection, &filter).unwrap();
        assert_eq!(dashboard, expected);
        assert_eq!(
            connection
                .pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let transaction = connection.transaction().unwrap();
        let detail = Store::group_from(&transaction, "g", &filter)
            .unwrap()
            .unwrap();
        assert_eq!(detail, expected_detail);
        assert_eq!(
            transaction
                .pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    assert_eq!(
        store
            .request("synthetic-large-definition")
            .unwrap()
            .unwrap()["answers"][0]["definition"],
        saved["answers"][0]["definition"]
    );
    assert_eq!(plaintext_database(&path).unwrap(), !encrypted);
}

#[test]
fn encrypted_dashboard_temporary_payloads_stay_in_memory() {
    dashboard_temp_storage_case(true);
}

#[test]
fn demo_dashboard_temporary_payloads_stay_in_memory() {
    dashboard_temp_storage_case(false);
}

#[test]
fn unsupported_schema_is_rejected_before_changing_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let conn = store.writer_connection().unwrap();
    conn.execute_batch(
        "CREATE INDEX answer_group ON answers(group_id,request_id);
            UPDATE schema_version SET version=999;
            PRAGMA journal_mode=DELETE;",
    )
    .unwrap();
    drop(conn);
    drop(store);
    let before = std::fs::read(&path).unwrap();
    let error = Store::open(&path, 7, 100)
        .err()
        .expect("Future schema must be rejected");
    assert!(error.to_string().contains("Unsupported database schema"));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "Failed startup must not change schema, data, or journal mode"
    );
}

#[test]
fn unsupported_schema_in_crash_left_wal_preserves_database_and_wal_bytes() {
    for encrypted in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.sqlite");
        let crashed = directory.path().join("crashed.sqlite");
        let key = "e".repeat(64);
        let store = if encrypted {
            Store::open_encrypted(&original, key.clone(), 7, 100)
        } else {
            Store::open(&original, 7, 100)
        }
        .unwrap();
        let writer = store.writer_connection().unwrap();
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let main = std::fs::read(&original).unwrap();
        writer
            .execute("UPDATE schema_version SET version=999", [])
            .unwrap();
        assert_eq!(
            std::fs::read(&original).unwrap(),
            main,
            "The unsupported version must exist only in the committed WAL"
        );
        let peer = |path: &Path, suffix: &str| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            PathBuf::from(name)
        };
        // The writer is quiescent. Copying its complete file set leaves a
        // fixture with committed WAL state and no live SQLite connection.
        for suffix in ["", "-wal", "-shm"] {
            std::fs::copy(peer(&original, suffix), peer(&crashed, suffix)).unwrap();
        }
        drop(writer);
        let before: Vec<_> = ["", "-wal"]
            .into_iter()
            .map(|suffix| {
                let bytes = std::fs::read(peer(&crashed, suffix)).unwrap();
                assert!(!bytes.is_empty());
                (suffix, sha2::Sha256::digest(bytes))
            })
            .collect();
        let error = if encrypted {
            Store::open_encrypted(&crashed, key, 7, 100)
        } else {
            Store::open(&crashed, 7, 100)
        }
        .err()
        .expect("Committed future schema must be rejected");
        assert!(
            error
                .to_string()
                .contains("Unsupported database schema [999]")
        );
        for (suffix, digest) in before {
            assert_eq!(
                sha2::Sha256::digest(std::fs::read(peer(&crashed, suffix)).unwrap()),
                digest,
                "Rejected startup must preserve {suffix:?} data bytes (encrypted={encrypted})"
            );
        }
        // SHM is a transient WAL index; SQLite may rebuild it while reading
        // a crash-left WAL. Its bytes are not persistent database content.
    }
}

#[test]
fn unsupported_plaintext_and_encrypted_schemas_survive_live_startup_unchanged() {
    for encrypted in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("future.sqlite");
        let key = "d".repeat(64);
        let store = if encrypted {
            Store::open_encrypted(&path, key.clone(), 7, 100).unwrap()
        } else {
            Store::open(&path, 7, 100).unwrap()
        };
        let conn = store.writer_connection().unwrap();
        conn.execute_batch("UPDATE schema_version SET version=999; PRAGMA journal_mode=DELETE;")
            .unwrap();
        drop(conn);
        drop(store);
        let before = std::fs::read(&path).unwrap();
        let error = Store::open_encrypted(&path, key, 7, 100).err().unwrap();
        assert!(error.to_string().contains("Unsupported database schema"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(plaintext_database(&path).unwrap(), !encrypted);
    }
}

#[test]
fn dashboard_filters_select_whole_requests_without_cross_query_leakage() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("search.sqlite"), 7, 100).unwrap();
    let mut first = record("first");
    let mut companion = first["answers"][0].clone();
    companion["key"] = json!("followup");
    companion["group_id"] = json!("other-group");
    companion["value"] = json!(0.2);
    first["answers"]
        .as_array_mut()
        .unwrap()
        .push(companion.clone());
    let mut source_match = record("second");
    source_match["source"] = json!("URGENT archive");
    source_match["answers"] = json!([companion.clone()]);
    let mut id_match = record("urgent-id");
    id_match["answers"] = json!([companion.clone()]);
    let mut unmatched = record("fourth");
    unmatched["answers"] = json!([companion]);
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[first, source_match, id_match, unmatched],
        )
        .unwrap();

    let cases = [
        (json!({"search":"UrGeNt"}), 3, 4),
        (json!({"group":"g"}), 1, 2),
        (json!({"search":"first", "group":"g"}), 1, 2),
        (json!({"search":"second", "group":"g"}), 0, 0),
        (json!({"search":"%_"}), 0, 0),
        (json!({"search":""}), 4, 5),
        (json!({"search":"UrGeNt"}), 3, 4),
    ];
    for (case, requests, answers) in cases {
        let filter: Filter = serde_json::from_value(case.clone()).unwrap();
        let dashboard = store.dashboard(&filter).unwrap();
        assert_eq!(dashboard["summary"]["request_count"], requests, "{case}");
        assert_eq!(dashboard["summary"]["answer_count"], answers, "{case}");
        let grouped: i64 = dashboard["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["answer_count"].as_i64().unwrap())
            .sum();
        assert_eq!(grouped, answers, "{case}");
        assert_eq!(
            dashboard["requests"].as_array().unwrap().len(),
            requests as usize,
            "{case}"
        );
    }
    let schema_count: i64 = store
        .reader()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='dashboard_request_ids'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        schema_count, 0,
        "Request selections must not persist in the main database"
    );
}

#[test]
fn empty_group_filter_matches_an_omitted_filter_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty-group.sqlite"), 7, 100).unwrap();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[record("one")])
        .unwrap();
    let unfiltered = Filter {
        window: Some("all".into()),
        as_of: Some(Utc::now().timestamp_millis()),
        ..Default::default()
    };
    let empty = Filter {
        group: Some(String::new()),
        ..unfiltered.clone()
    };
    assert_eq!(
        store.dashboard(&empty).unwrap(),
        store.dashboard(&unfiltered).unwrap()
    );
    assert_eq!(
        store.group("g", &empty).unwrap(),
        store.group("g", &unfiltered).unwrap()
    );
    for format in ["jsonl", "csv"] {
        assert_eq!(
            store.export(&empty, format).unwrap(),
            store.export(&unfiltered, format).unwrap()
        );
    }
}

#[test]
fn group_detail_preserves_the_parent_scope_for_every_section() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("intersecting-groups.sqlite"), 7, 100).unwrap();
    let mut shared = record("shared-parent");
    let mut companion = shared["answers"][0].clone();
    companion["key"] = json!("followup");
    companion["group_id"] = json!("other-group");
    companion["family_id"] = json!("other-family");
    companion["family_name"] = json!("Followup family");
    shared["answers"]
        .as_array_mut()
        .unwrap()
        .push(companion.clone());
    let only_original = record("original-only");
    let mut only_companion = record("companion-only");
    only_companion["answers"] = json!([companion]);
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[shared, only_original, only_companion],
        )
        .unwrap();
    for (selected, requested, count) in [
        ("g", "other-group", 1),
        ("g", "other-family", 1),
        ("other-family", "g", 1),
        ("absent-group", "g", 0),
    ] {
        let detail = store
            .group(
                requested,
                &Filter {
                    window: Some("all".into()),
                    group: Some(selected.into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(detail["group"]["request_count"], count);
        assert_eq!(detail["total_answers"], count);
        for key in ["requests", "answers"] {
            let rows = detail[key].as_array().unwrap();
            assert_eq!(rows.len(), count, "{requested} within {selected}: {key}");
            if count > 0 {
                let id = if key == "requests" {
                    "id"
                } else {
                    "request_id"
                };
                assert_eq!(rows[0][id], "shared-parent");
            }
        }
        let timeline_count: i64 = detail["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bucket| bucket["requests"].as_i64().unwrap())
            .sum();
        assert_eq!(timeline_count, count as i64);
        for version in detail["versions"].as_array().unwrap() {
            assert_eq!(version["request_count"], count);
        }
    }
}

#[test]
fn strict_dashboard_counts_distinct_parents_and_only_valid_known_values() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("strict-groups.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut first = record("parent-a");
    first["timestamp"] = json!(now - 100);
    first["answers"][0]["confidence"] = json!(0.9);
    let mut additional = first["answers"][0].clone();
    additional["key"] = json!("second-key");
    additional["value"] = json!(0.2);
    additional["confidence"] = json!(0.3);
    first["answers"].as_array_mut().unwrap().push(additional);
    let mut invalid = record("parent-b");
    invalid["timestamp"] = json!(now);
    invalid["answers"][0]["valid"] = json!(false);
    invalid["answers"][0]["value"] = json!(1e308);
    invalid["answers"][0]["confidence"] = json!(0.99);
    let mut unknown_confidence = record("parent-c");
    unknown_confidence["timestamp"] = json!(now - 200);
    unknown_confidence["answers"][0]["value"] = json!(0.4);
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[first, invalid, unknown_confidence],
        )
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let overview = store.dashboard(&filter).unwrap();
    let group = &overview["groups"][0];
    assert_eq!(group["answer_count"], 4);
    assert_eq!(group["valid_count"], 3);
    assert_eq!(group["request_count"], 3);
    assert_eq!(group["last_seen"], now);
    assert_eq!(group["mean_confidence"], 0.6);
    assert_eq!(
        group["distribution"],
        json!([
            {"label":"0.2–<0.3","count":1},
            {"label":"0.4–<0.5","count":1},
            {"label":"0.8–<0.9","count":1}
        ])
    );
    assert_overview_matches_detail(group, &store.group("g", &filter).unwrap().unwrap()["group"]);
}

#[test]
fn dashboard_merges_latest_hundred_strict_groups_and_families_with_stable_ties() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("many-groups.sqlite"), 7, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut records = Vec::new();
    let mut expected = Vec::new();
    for index in 0..130 {
        let id = format!("strict-{index:03}");
        let timestamp = now - (index % 7) * 100;
        let mut request = record(&format!("strict-parent-{index:03}"));
        request["timestamp"] = json!(timestamp);
        request["answers"][0]["group_id"] = json!(id);
        records.push(request);
        expected.push((id, timestamp));
    }
    for index in 0..15 {
        let family = format!("family-{index:03}");
        let timestamp = now - (index % 7) * 100;
        let mut request = record(&format!("family-parent-{index:03}"));
        request["timestamp"] = json!(timestamp);
        let mut answers = Vec::new();
        for key in 0..2 {
            let mut answer = request["answers"][0].clone();
            answer["key"] = json!(format!("candidate-{key}"));
            answer["group_id"] = json!(format!("member-{index:03}-{key}"));
            answer["family_id"] = json!(family);
            answer["family_name"] = json!("Related questions");
            answer["valid"] = json!(key == 0 || index % 3 != 0);
            answers.push(answer);
        }
        request["answers"] = json!(answers);
        records.push(request);
        expected.push((family, timestamp));
    }
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    expected.sort_by(|(left_id, left_time), (right_id, right_time)| {
        right_time
            .cmp(left_time)
            .then_with(|| left_id.cmp(right_id))
    });
    expected.truncate(100);
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let overview = store.dashboard(&filter).unwrap();
    let groups = overview["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 100);
    assert_eq!(overview["group_limit"], 100);
    assert_eq!(
        groups
            .iter()
            .map(|group| text(group, "id"))
            .collect::<Vec<_>>(),
        expected.into_iter().map(|(id, _)| id).collect::<Vec<_>>()
    );
    assert!(groups.iter().any(|group| group["is_family"] == true));
    assert!(groups.iter().any(|group| group["is_family"] == false));
    let connection = store.reader().unwrap();
    for group in groups {
        let id = text(group, "id");
        let metadata = Store::group_metadata(&connection, &id, &mut VersionCounts::new())
            .unwrap()
            .unwrap();
        let (baseline, _) = Store::group_summary(&connection, &id, &filter, 0, metadata).unwrap();
        assert_overview_matches_detail(group, &baseline);
        assert_eq!(group["request_count"], 1);
        if group["is_family"] == true {
            assert_eq!(group["answer_count"], 2);
        }
    }
    let narrowed = Filter {
        window: Some("1h".into()),
        source: Some("test".into()),
        search: Some("strict-parent-129".into()),
        ..Default::default()
    };
    let overview = store.dashboard(&narrowed).unwrap();
    assert_eq!(overview["groups"].as_array().unwrap().len(), 1);
    assert_eq!(overview["groups"][0]["id"], "strict-129");
    assert_overview_matches_detail(
        &overview["groups"][0],
        &Store::group_summary(
            &connection,
            "strict-129",
            &narrowed,
            0,
            Store::group_metadata(&connection, "strict-129", &mut VersionCounts::new())
                .unwrap()
                .unwrap(),
        )
        .unwrap()
        .0,
    );
    let missing_source = Filter {
        source: Some("absent".into()),
        ..Default::default()
    };
    assert!(
        store.dashboard(&missing_source).unwrap()["groups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn overview_and_detail_agree_for_mixed_definitions_and_families() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite"), 7, 10_000).unwrap();
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &crate::model::sample_records(),
        )
        .unwrap();
    let baseline = store
        .dashboard(&Filter {
            window: Some("all".into()),
            ..Default::default()
        })
        .unwrap();
    let family = baseline["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["is_family"] == true)
        .unwrap();
    assert!(family["answer_count"].as_i64().unwrap() > family["request_count"].as_i64().unwrap());
    let strict = baseline["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["is_family"] == false)
        .unwrap();
    let cases = [
        json!({"window":"all"}),
        json!({"window":"all","source":"Code search"}),
        json!({"window":"all","source":"Support inbox"}),
        json!({"window":"all","model":"jev-synthetic-b"}),
        json!({"window":"1h"}),
        json!({"window":"24h"}),
        json!({"window":"7d"}),
        json!({"window":"all","status":"error"}),
        json!({"window":"all","search":"urgency"}),
        json!({"window":"all","group":family["id"]}),
        json!({"window":"all","group":strict["id"]}),
        json!({"window":"24h","source":"Code search","model":"jev-synthetic-b","search":"c0"}),
    ];
    for case in cases {
        let mut filter: Filter = serde_json::from_value(case.clone()).unwrap();
        filter.as_of = Some(Utc::now().timestamp_millis());
        let overview = store.dashboard(&filter).unwrap();
        assert!(
            !overview["groups"].as_array().unwrap().is_empty(),
            "empty filter fixture: {case}"
        );
        for group in overview["groups"].as_array().unwrap() {
            let detail = store
                .group(group["id"].as_str().unwrap(), &filter)
                .unwrap()
                .unwrap();
            assert_overview_matches_detail(group, &detail["group"]);
            let total: i64 = group["distribution"]
                .as_array()
                .unwrap()
                .iter()
                .map(|bin| bin["count"].as_i64().unwrap())
                .sum();
            assert_eq!(total, group["valid_count"].as_i64().unwrap());
            assert!(
                group["request_count"].as_i64().unwrap() <= group["answer_count"].as_i64().unwrap()
            );
        }
    }
}

#[test]
fn bounded_windows_use_both_frozen_boundaries_and_all_keeps_future_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("window.sqlite"), 14, 100).unwrap();
    let as_of = Utc::now().timestamp_millis() - 60_000;
    let records: Vec<_> = [
        ("older", as_of - 604_800_001),
        ("week", as_of - 604_800_000),
        ("day", as_of - 86_400_000),
        ("hour", as_of - 3_600_000),
        ("end", as_of),
        ("future", as_of + 1),
    ]
    .into_iter()
    .map(|(id, timestamp)| {
        let mut row = record(id);
        row["timestamp"] = json!(timestamp);
        row
    })
    .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    for (window, count) in [("1h", 2), ("24h", 3), ("7d", 4), ("all", 6)] {
        let filter = Filter {
            window: Some(window.into()),
            as_of: Some(as_of),
            ..Default::default()
        };
        let overview = store.dashboard(&filter).unwrap();
        assert_eq!(overview["summary"]["request_count"], count, "{window}");
        assert_eq!(overview["groups"][0]["request_count"], count, "{window}");
        assert_overview_matches_detail(
            &overview["groups"][0],
            &store.group("g", &filter).unwrap().unwrap()["group"],
        );
        assert_eq!(
            store.export(&filter, "jsonl").unwrap().lines().count(),
            count as usize
        );
    }
}

#[test]
fn ledger_counts_once_and_import_identity_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
    let mut conn = store.writer_connection().unwrap();
    let mut first = record("1");
    first["source_event_id"] = json!("original");
    first["import_format"] = json!("observer-jsonl");
    let mut duplicate = first.clone();
    duplicate["id"] = json!("new-id");
    let second = record("2");
    assert_eq!(
        store
            .write_batch(&mut conn, &[first, duplicate, second])
            .unwrap(),
        2
    );
    let d = store.dashboard(&Filter::default()).unwrap();
    assert_eq!(d["summary"]["request_count"], 2);
    assert_eq!(d["summary"]["cost_usd"], 0.2);
    assert_eq!(d["groups"][0]["valid_count"], 2);
}

#[test]
fn reopening_removes_legacy_index_without_changing_stored_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    writer
        .execute_batch("CREATE INDEX answer_group ON answers(group_id,request_id)")
        .unwrap();
    store.write_batch(&mut writer, &[record("kept")]).unwrap();
    store.add_label("kept", "urgent", "correct").unwrap();
    let before = store.request("kept").unwrap().unwrap();
    drop(writer);
    let reopened = Store::open(&path, 7, 100).unwrap();
    assert_eq!(reopened.request("kept").unwrap().unwrap(), before);
    let connection = reopened.writer_connection().unwrap();
    let legacy_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name='answer_group'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 0);
    let overview = reopened.dashboard(&Filter::default()).unwrap();
    assert_eq!(overview["summary"]["request_count"], 1);
    assert_eq!(overview["groups"][0]["valid_count"], 1);
    assert_overview_matches_detail(
        &overview["groups"][0],
        &reopened.group("g", &Filter::default()).unwrap().unwrap()["group"],
    );
}
#[test]
fn labels_roundtrip_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[record("1")])
        .unwrap();
    assert_eq!(
        store.add_label("absent", "urgent", "correct").unwrap(),
        LabelOutcome::RequestNotFound
    );
    assert_eq!(
        store.add_label("1", "absent", "correct").unwrap(),
        LabelOutcome::AnswerNotFound
    );
    assert!(store.add_label("1", "urgent", "invalid").is_err());
    assert_eq!(
        store.add_label("1", "urgent", "incorrect").unwrap(),
        LabelOutcome::Saved
    );
    assert_eq!(
        store.request("1").unwrap().unwrap()["labels"][0]["label"],
        "incorrect"
    );
    assert!(
        store
            .export(&Filter::default(), "jsonl")
            .unwrap()
            .contains("incorrect")
    );
    assert_eq!(
        store.add_label("1", "urgent", "correct").unwrap(),
        LabelOutcome::Saved
    );
    let reviewed = store.request("1").unwrap().unwrap();
    assert_eq!(reviewed["labels"].as_array().unwrap().len(), 1);
    assert_eq!(reviewed["labels"][0]["label"], "correct");
    assert_eq!(reviewed["labels"][0]["source"], "local-reviewer");
    assert!(reviewed["labels"][0]["timestamp"].is_number());
    store.delete_all().unwrap();
    assert!(store.request("1").unwrap().is_none());
    assert_eq!(
        store.add_label("1", "urgent", "unknown").unwrap(),
        LabelOutcome::RequestNotFound
    );
    let remaining: i64 = store
        .writer_connection()
        .unwrap()
        .query_row("SELECT count(*) FROM labels", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(
        store.dashboard(&Filter::default()).unwrap()["summary"]["request_count"],
        0
    );
}

#[test]
fn label_classification_waits_for_a_pending_parent_deletion() {
    use std::{cell::RefCell, sync::mpsc, thread};
    thread_local! {
        static BUSY_SIGNAL: RefCell<Option<mpsc::SyncSender<()>>> = const { RefCell::new(None) };
    }
    fn retry_busy(attempt: i32) -> bool {
        BUSY_SIGNAL.with(|signal| {
            if let Some(sender) = signal.borrow_mut().take() {
                let _ = sender.send(());
            }
        });
        thread::sleep(Duration::from_millis(1));
        attempt < 2000
    }

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("label-delete.sqlite"), 7, 100).unwrap();
    let mut deleting = store.writer_connection().unwrap();
    store.write_batch(&mut deleting, &[record("1")]).unwrap();
    let mut labeling = store.writer_connection().unwrap();
    let deletion = deleting
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    deletion
        .execute("DELETE FROM requests WHERE id='1'", [])
        .unwrap();
    let (busy_tx, busy_rx) = mpsc::sync_channel(1);
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        BUSY_SIGNAL.with(|signal| *signal.borrow_mut() = Some(busy_tx));
        labeling.busy_handler(Some(retry_busy)).unwrap();
        result_tx
            .send(Store::add_label_from(
                &mut labeling,
                "1",
                "urgent",
                "correct",
            ))
            .unwrap();
    });
    // Observe the actual writer-lock conflict before committing deletion;
    // no sleep-based assumption about thread scheduling is needed.
    busy_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    deletion.commit().unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        LabelOutcome::RequestNotFound
    );
    worker.join().unwrap();
    let labels: i64 = deleting
        .query_row("SELECT count(*) FROM labels", [], |row| row.get(0))
        .unwrap();
    assert_eq!(labels, 0);
}
#[test]
fn unknown_cost_is_not_zero() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite"), 7, 100).unwrap();
    let mut r = record("1");
    r["cost_usd"] = Value::Null;
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[r])
        .unwrap();
    let d = store.dashboard(&Filter::default()).unwrap();
    assert!(d["summary"]["cost_usd"].is_null());
    assert_eq!(d["summary"]["cost_known_requests"], 0);
}

#[test]
fn unknown_model_stays_null_in_request_feed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("unknown.sqlite"), 7, 100).unwrap();
    let mut row = record("unknown-model");
    row["model"] = Value::Null;
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[row])
        .unwrap();
    let dashboard = store.dashboard(&Filter::default()).unwrap();
    assert!(dashboard["requests"][0]["model"].is_null());
    assert!(store.request("unknown-model").unwrap().unwrap()["model"].is_null());
}

#[test]
fn restart_applies_age_and_count_retention_before_exposing_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("retention.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let mut old = record("old");
    old["timestamp"] = json!(Utc::now().timestamp_millis() - 2 * 86_400_000);
    old["answers"][0]["group_id"] = json!("old-group");
    store
        .write_batch(
            &mut store.writer_connection().unwrap(),
            &[old, record("second"), record("latest")],
        )
        .unwrap();
    assert!(store.request("old").unwrap().is_some());
    let reopened = Store::open(&path, 1, 1).unwrap();
    assert!(reopened.request("old").unwrap().is_none());
    assert!(reopened.request("second").unwrap().is_none());
    assert!(reopened.request("latest").unwrap().is_some());
    let connection = reopened.writer_connection().unwrap();
    let orphans: i64 = connection
        .query_row(
            "SELECT count(*) FROM groups WHERE id='old-group'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
}

#[test]
fn export_pages_share_an_explicit_time_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("export.sqlite"), 7, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..205)
        .map(|index| {
            let mut row = record(&format!("record-{index}"));
            row["timestamp"] = json!(now - 3_630_000);
            row
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    // All rows were within the hour at export start, but the moving wall
    // clock would exclude them. No timing-dependent sleeps are needed.
    let filter = Filter {
        window: Some("1h".into()),
        as_of: Some(now - 60_000),
        ..Default::default()
    };
    let mut session = store.start_export(&filter, "jsonl").unwrap();
    let (first, _, _, more) = session.next_page().unwrap();
    assert_eq!(first.lines().count(), 100);
    assert!(more);
    let (second, _, _, more) = session.next_page().unwrap();
    assert_eq!(second.lines().count(), 100);
    assert!(more);
    let (last, _, _, more) = session.next_page().unwrap();
    assert_eq!(last.lines().count(), 5);
    assert!(!more);
    let external: Filter =
        serde_json::from_value(json!({"window":"1h","as_of":now-60_000})).unwrap();
    assert!(external.as_of.is_none());
    assert!(
        store
            .start_export(&external, "jsonl")
            .unwrap()
            .next_page()
            .unwrap()
            .0
            .is_empty()
    );
}

type ParentReadHook = Box<dyn FnOnce(&str)>;

thread_local! {
    static AFTER_PARENT_READ: std::cell::RefCell<Option<ParentReadHook>> = const {
        std::cell::RefCell::new(None)
    };
}

pub(super) fn after_parent_read(id: &str) {
    // Release the borrow before invoking the hook so it can make other reads.
    let hook = AFTER_PARENT_READ.with_borrow_mut(Option::take);
    if let Some(hook) = hook {
        hook(id);
    }
}

fn with_parent_read_hook<T>(hook: ParentReadHook, read: impl FnOnce() -> T) -> T {
    struct ResetHook;
    impl Drop for ResetHook {
        fn drop(&mut self) {
            AFTER_PARENT_READ.with_borrow_mut(|hook| *hook = None);
        }
    }

    // Each test thread owns its hook, and unwinding cannot leak it to a
    // subsequent test. Fail immediately if a test accidentally nests hooks.
    AFTER_PARENT_READ.with_borrow_mut(|current| {
        assert!(current.is_none(), "A parent-read hook is already installed");
        *current = Some(hook);
    });
    let _reset = ResetHook;
    let result = read();
    AFTER_PARENT_READ.with_borrow(|hook| {
        assert!(hook.is_none(), "The public read did not consume its hook");
    });
    result
}

#[derive(Clone, Copy, Debug)]
enum SnapshotMutation {
    Delete,
    Relabel,
}

fn assert_parent_snapshot(mutation: SnapshotMutation, read: impl Fn(&Store) -> Value) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("read-snapshot.sqlite"), 7, 1000).unwrap();
    let mut writer = store.writer_connection().unwrap();
    store
        .write_batch(&mut writer, &[record("reviewed")])
        .unwrap();
    store.add_label("reviewed", "urgent", "correct").unwrap();
    let before = store.request("reviewed").unwrap().unwrap();
    assert_eq!(before["answers"].as_array().unwrap().len(), 1);
    assert_eq!(before["labels"][0]["label"], "correct");
    let expected = read(&store);
    let concurrent_store = store.clone();
    let observed = with_parent_read_hook(
        Box::new(move |id| {
            assert_eq!(id, "reviewed");
            // Commit on another connection after the public API reads the
            // parent, before it reads any answers or labels. Joining makes
            // the schedule deterministic without sleeps or polling.
            std::thread::spawn(move || {
                let mut connection = concurrent_store.writer_connection().unwrap();
                let tx = connection.transaction().unwrap();
                let changed = match mutation {
                    SnapshotMutation::Delete => tx
                        .execute("DELETE FROM requests WHERE id='reviewed'", [])
                        .unwrap(),
                    SnapshotMutation::Relabel => tx
                        .execute(
                            "UPDATE labels SET label='incorrect' WHERE request_id='reviewed' AND key='urgent'",
                            [],
                        )
                        .unwrap(),
                };
                assert_eq!(changed, 1);
                tx.commit().unwrap();
            })
            .join()
            .unwrap();
        }),
        || read(&store),
    );
    assert_eq!(observed, expected, "Mixed snapshots during {mutation:?}");

    let after = store.request("reviewed").unwrap();
    match mutation {
        SnapshotMutation::Delete => assert!(after.is_none()),
        SnapshotMutation::Relabel => {
            assert_eq!(after.unwrap()["labels"][0]["label"], "incorrect");
        }
    }
    let busy: i64 = writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .unwrap();
    assert_eq!(busy, 0, "A completed read must release its snapshot");
}

fn snapshot_export(store: &Store, format: &str) -> Value {
    let filter = Filter {
        window: Some("all".into()),
        source: Some("test".into()),
        ..Default::default()
    };
    let (page, cursor, through, more) = store
        .start_export(&filter, format)
        .unwrap()
        .next_page()
        .unwrap();
    assert!(cursor > 0);
    assert_eq!(cursor, through);
    assert!(!more);
    if format == "jsonl" {
        serde_json::from_str(page.trim()).unwrap()
    } else {
        json!(page)
    }
}

#[test]
fn request_keeps_parent_snapshot_during_concurrent_delete() {
    assert_parent_snapshot(SnapshotMutation::Delete, |store| {
        store.request("reviewed").unwrap().unwrap()
    });
}

#[test]
fn request_keeps_parent_snapshot_during_concurrent_label_edit() {
    assert_parent_snapshot(SnapshotMutation::Relabel, |store| {
        store.request("reviewed").unwrap().unwrap()
    });
}

#[test]
fn jsonl_export_keeps_parent_snapshot_during_concurrent_delete() {
    assert_parent_snapshot(SnapshotMutation::Delete, |store| {
        snapshot_export(store, "jsonl")
    });
}

#[test]
fn jsonl_export_keeps_parent_snapshot_during_concurrent_label_edit() {
    assert_parent_snapshot(SnapshotMutation::Relabel, |store| {
        snapshot_export(store, "jsonl")
    });
}

#[test]
fn csv_export_keeps_parent_snapshot_during_concurrent_delete() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("csv-page-snapshot.sqlite"), 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    store
        .write_batch(&mut writer, &[record("first"), record("second")])
        .unwrap();
    let expected = snapshot_export(&store, "csv");
    let concurrent = store.clone();
    let observed = with_parent_read_hook(
        Box::new(move |id| {
            assert_eq!(id, "first");
            // CSV needs only one SELECT per parent. Delete the next parent as
            // well to verify the whole page shares the selected IDs' snapshot.
            std::thread::spawn(move || {
                assert_eq!(
                    concurrent
                        .writer_connection()
                        .unwrap()
                        .execute("DELETE FROM requests", [])
                        .unwrap(),
                    2
                );
            })
            .join()
            .unwrap();
        }),
        || snapshot_export(&store, "csv"),
    );
    assert_eq!(observed, expected);
    assert!(store.is_empty().unwrap());
    let busy: i64 = writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .unwrap();
    assert_eq!(busy, 0, "A completed export page must release its snapshot");
}

#[test]
fn csv_export_preserves_unknowns_escaping_and_all_answer_counts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("csv-summary.sqlite"), 7, 100).unwrap();
    let mut request = record("=ID(\"x\")");
    request["timestamp"] = Value::Null;
    request["imported_at"] = json!(Utc::now().timestamp_millis());
    request["source"] = json!("line,\n\"source\"");
    request["model"] = Value::Null;
    let mut invalid = request["answers"][0].clone();
    invalid["key"] = json!("invalid");
    invalid["valid"] = json!(false);
    invalid["value"] = Value::Null;
    request["answers"].as_array_mut().unwrap().push(invalid);
    request["labels"] = json!([{"key":"urgent","label":"correct"}]);
    let mut action = record("action");
    action["event_kind"] = json!("application_action");
    action["answers"] = json!([]);
    action["imported_at"] = request["imported_at"].clone();
    for key in [
        "timestamp",
        "model",
        "status",
        "duration_ms",
        "input_tokens",
        "output_tokens",
        "cost_usd",
        "cost_basis",
    ] {
        action[key] = Value::Null;
    }
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[request, action])
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    assert_eq!(
        store.export(&filter, "csv").unwrap(),
        concat!(
            "id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,event_kind\n",
            "\"'=ID(\"\"x\"\")\",\"\",\"line,\n\"\"source\"\"\",\"\",\"200\",\"4.0\",\"100\",\"3\",\"0.1\",\"synthetic\",\"2\",\"request\"\n",
            "\"action\",\"\",\"test\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"0\",\"application_action\"\n"
        )
    );
}

#[test]
fn csv_cells_neutralize_disguised_formulas() {
    for value in [" =1+1", "\n=1+1", "\t=1+1", "＝1+1", " ＠SUM(1)", "\0=1+1"] {
        assert!(csv_cell(&json!(value)).starts_with("\"'"), "{value:?}");
    }
    assert_eq!(csv_cell(&json!("ordinary text")), "\"ordinary text\"");
}

#[test]
fn group_observations_keep_unknown_event_time_and_break_ties_by_newest_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("group-observations.sqlite"), 7, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..105)
        .map(|index| {
            let mut row = record(&format!("request-{index:03}"));
            row["timestamp"] = json!(now);
            row
        })
        .collect();
    let mut writer = store.writer_connection().unwrap();
    store.write_batch(&mut writer, &records).unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let details = store.group("g", &filter).unwrap().unwrap();
    assert_eq!(details["requests"][0]["id"], "request-104");
    assert_eq!(details["answers"][0]["request_id"], "request-104");
    assert_eq!(details["answers"][99]["request_id"], "request-005");

    let mut imported = record("untimed");
    imported["timestamp"] = Value::Null;
    imported["imported_at"] = json!(now + 1000);
    store.write_batch(&mut writer, &[imported]).unwrap();
    let details = store.group("g", &filter).unwrap().unwrap();
    assert_eq!(details["answers"][0]["request_id"], "untimed");
    assert!(
        details["answers"][0]["timestamp"].is_null(),
        "Import time is not an original event timestamp"
    );
    assert_eq!(details["answers"][0]["imported_at"], now + 1000);
}

#[cfg(unix)]
#[test]
fn database_and_live_wal_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    let mut connection = store.writer_connection().unwrap();
    store
        .write_batch(&mut connection, &[record("private")])
        .unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join(format!("private.sqlite{suffix}"));
        let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{suffix}");
    }
}

#[cfg(unix)]
#[test]
fn reopening_secures_existing_wal_files_through_a_database_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.sqlite");
    let store = Store::open(&path, 7, 100).unwrap();
    // Keep a connection alive so existing WAL/SHM files survive reopening.
    let mut connection = store.writer_connection().unwrap();
    store
        .write_batch(&mut connection, &[record("private-existing")])
        .unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join(format!("existing.sqlite{suffix}"));
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let alias = dir.path().join("database-link.sqlite");
    symlink(&path, &alias).unwrap();
    let reopened = Store::open(&alias, 7, 100).unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let file = dir.path().join(format!("existing.sqlite{suffix}"));
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600,
            "existing {suffix} remained readable by other users"
        );
    }
    assert!(reopened.request("private-existing").unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn opening_rejects_symlinked_database_peers_without_changing_the_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for suffix in ["-wal", "-shm", "-journal"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.sqlite");
        Store::open(&path, 7, 100).unwrap();
        let unrelated = dir.path().join("unrelated.txt");
        std::fs::write(&unrelated, "unchanged").unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(
            &unrelated,
            dir.path().join(format!("private.sqlite{suffix}")),
        )
        .unwrap();
        assert!(
            Store::open(&path, 7, 100).is_err(),
            "accepted {suffix} symlink"
        );
        assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "unchanged");
        assert_eq!(
            std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}
