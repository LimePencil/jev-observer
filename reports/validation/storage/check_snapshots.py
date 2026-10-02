#!/usr/bin/env python3
"""Exercise public Store read APIs with an injected deterministic writer schedule.

This is an instrumented-source reproduction, NOT an unmodified-binary race test.
Copies archived baseline/current store.rs and adds a single hook after parent SELECT.
The hook waits for a separate writer thread to commit its mutation, then lets
the original unmodified request()/export_page() control flow proceed.
No author tests are compiled or run; dependency artifacts are reused by rustc.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
OUT = ROOT / ".jev-observer/independent-validation-rerun/storage"
OUT.mkdir(parents=True, exist_ok=True)
DEPS = pathlib.Path(os.environ.get("JEV_VALIDATION_DEPS", ROOT / "target/debug/deps"))
BASE_REV = os.environ.get("JEV_VALIDATION_BASE_REV", "90e3cfb625d85b4279977a690903b7790468f7ed")
HOOK_MARKER = '''        let Some(mut record) = record else {
            return Ok(None);
        };'''
PARENT_MARKER = '''        #[cfg(test)]
        if record.is_some() {
            tests::after_parent_read(id);
        }'''
HARNESS = r'''
#![allow(dead_code)]
use serde_json::{json, Value};
use std::sync::Mutex;

mod store { include!("STORE_SOURCE"); }
static HOOK: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);
fn after_parent_read(_id: &str) {
    let hook = HOOK.lock().unwrap().take();
    if let Some(hook) = hook { hook(); }
}
fn record(generation: &str) -> Value {
    let label = if generation == "A" { "correct" } else { "incorrect" };
    json!({
        "schema_version": 1, "id": "race", "generation": generation,
        "timestamp": chrono::Utc::now().timestamp_millis(), "source": "independent",
        "model": "test", "status": 200, "capture_complete": true,
        "answers": [{"key": "flag", "group_id": "g", "kind": "noul",
            "valid": true, "value": 0.7, "generation": generation,
            "definition_id": "definition", "presentation_id": "presentation",
            "definition": {"type": "noul", "instructions": "Flag?"}}],
        "labels": [{"key": "flag", "label": label, "source": "independent"}]
    })
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let store = store::Store::open(std::path::Path::new(&args[1]), 7, 1000)?;
    let mut writer = store.writer_connection()?;
    store.write_batch(&mut writer, &[record("A")])?;
    let filter = store::Filter { window: Some("all".into()), ..Default::default() };
    let before = store.request("race")?.unwrap();
    let concurrent_store = store.clone();
    let mutation = args[3].clone();
    *HOOK.lock().unwrap() = Some(Box::new(move || {
        std::thread::spawn(move || {
            let mut connection = concurrent_store.writer_connection().unwrap();
            connection.execute("DELETE FROM requests WHERE id='race'", []).unwrap();
            if mutation == "replace" {
                concurrent_store.write_batch(&mut connection, &[record("B")]).unwrap();
            }
        }).join().unwrap();
    }));
    let observed = match args[2].as_str() {
        "request" => store.request("race")?.unwrap(),
        "export-jsonl" => {
            let (page, _, _, _) = store.export_page(&filter, "jsonl", 0, None)?;
            serde_json::from_str(page.trim())?
        }
        "export-csv" => {
            let (page, _, _, _) = store.export_page(&filter, "csv", 0, None)?;
            json!({"csv": page})
        }
        _ => anyhow::bail!("Unknown operation"),
    };
    let after = store.request("race")?;
    let checkpoint_busy: i64 = writer.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    println!("{}", json!({
        "operation": args[2], "mutation": args[3],
        "before": before, "observed": observed, "after": after,
        "completed_read_checkpoint_busy": checkpoint_busy,
        "hook_consumed": HOOK.lock().unwrap().is_none(),
    }));
    Ok(())
}
'''

base_source = os.environ.get("JEV_VALIDATION_BASE_SOURCE")
if base_source:
    baseline = pathlib.Path(base_source).read_text()
    manifest = {"head": "archived-source", "baseline_source_sha256": hashlib.sha256(baseline.encode()).hexdigest()}
else:
    resolved = subprocess.run(["git", "rev-parse", "--verify", BASE_REV + "^{commit}"],
                              cwd=ROOT, text=True, capture_output=True)
    if resolved.returncode:
        raise SystemExit("The fresh Git history excludes the prelaunch baseline. "
                         "Set JEV_VALIDATION_BASE_SOURCE to an archived store.rs, "
                         "or JEV_VALIDATION_BASE_REV to an available baseline commit.")
    baseline = subprocess.check_output(["git", "show", BASE_REV + ":src/store.rs"], cwd=ROOT, text=True)
    manifest = {"head": resolved.stdout.strip()}
results = []
for version in ("head", "working"):
    source = baseline if version == "head" else (ROOT / "src/store.rs").read_text()
    assert source.count(HOOK_MARKER) + source.count(PARENT_MARKER) == 1
    manifest[version + "_store_sha256"] = hashlib.sha256(source.encode()).hexdigest()
    (OUT / (version + ".original.rs")).write_text(source)
    hooked = OUT / (version + ".hooked.rs")
    if HOOK_MARKER in source:
        instrumented = source.replace(HOOK_MARKER, HOOK_MARKER + "\n        crate::after_parent_read(id);")
    else:
        instrumented = source.replace(PARENT_MARKER, "        if record.is_some() { crate::after_parent_read(id); }\n" + PARENT_MARKER)
    hooked.write_text(instrumented)
    harness = OUT / (version + ".harness.rs")
    harness.write_text(HARNESS.replace("STORE_SOURCE", str(hooked)))
    binary = OUT / (version + "-snapshot-harness")
    command = ["rustc", "--edition=2024", "--crate-name", "storage_snapshot_" + version,
               str(harness), "-o", str(binary), "-L", "dependency=" + str(DEPS)]
    for crate in ("anyhow", "chrono", "rusqlite", "serde", "serde_json"):
        candidates = list(DEPS.glob("lib" + crate + "-*.rlib"))
        assert len(candidates) == 1, candidates
        command += ["--extern", crate + "=" + str(candidates[0])]
    subprocess.run(command, check=True, cwd=ROOT)
    for operation in ("request", "export-jsonl", "export-csv"):
        for mutation in ("delete", "replace"):
            with tempfile.TemporaryDirectory(prefix="observer-read-snapshot-") as directory:
                result = json.loads(subprocess.check_output(
                    [str(binary), directory + "/history.sqlite", operation, mutation], text=True, cwd=ROOT))
            result["version"] = version
            results.append(result)
            if operation == "export-csv":
                result["observed_answer_count"] = result["observed"]["csv"].splitlines()[1].split(",")[10]
            else:
                result["observed_parent_generation"] = result["observed"]["generation"]
                result["observed_answer_generations"] = [a["generation"] for a in result["observed"]["answers"]]
                result["observed_labels"] = [label["label"] for label in result["observed"]["labels"]]
            print(json.dumps({k: v for k, v in result.items() if k not in ("before", "observed", "after")}))
manifest["evidence"] = "Instrumented exact source; public API call; hook after parent SELECT allows separate writer thread to commit deletion/replacement. Not an unmodified release-binary race."
(OUT / "snapshots-results.json").write_text(json.dumps({"manifest": manifest, "results": results}, indent=2) + "\n")
