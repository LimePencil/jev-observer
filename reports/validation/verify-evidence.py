#!/usr/bin/env python3
"""Check committed historical evidence, without rerunning the application."""
import hashlib
import json
from pathlib import Path
import runpy

ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads((ROOT / name).read_text())


for name, expected in read("evidence-manifest.json")["files"].items():
    if expected.get("privacy_normalized"):
        assert not expected["unchanged"], name
        data = (ROOT / name).read_bytes()
        assert len(data) == expected["published_bytes"], name
        assert hashlib.sha256(data).hexdigest() == expected["published_sha256"], name
    elif expected["unchanged"]:
        data = (ROOT / name).read_bytes()
        assert len(data) == expected["original_bytes"], name
        assert hashlib.sha256(data).hexdigest() == expected["original_sha256"], name

hashes = ["ec5f0ae0f5ca6ef1f8267bb5dc4560747096736638a01c67a6bd5ec445d7c047",
          "fd4c31dd90ada212e59fab18cdec59928a82b8f382c595bbe2f7eb81e0d3869c"]
for index, label in enumerate(("baseline", "current")):
    suite = read("backend/evidence.json")["suites"][label]
    current = label == "current"
    assert suite["sha256"] == suite["sha256_after"] == hashes[index]
    assert suite["shutdown_returncode"] == 0
    assert len(suite["probes"]) == 38
    for probe in suite["probes"]:
        claim, case = probe["claim"], probe["case"]
        if claim == "import_envelope":
            expected = 413 if case == "above_8m_plain" else 200 if current or case == "small_control" else 413
            assert probe["response"]["status"] == expected, (label, case)
            if case == "5000_small_records_below_8m":
                assert (probe["decoded_text_bytes"], probe["transport_bytes"]) == (8320000, 8755037)
                if current:
                    assert probe["response"]["body"]["imported"] == 5000
                    assert probe["dashboard_summary"]["request_count"] == 5000
        elif claim == "unknown_timestamp":
            if case == "omitted_timestamp_field":
                assert probe["response"]["status"] == 400
            elif case in ("unknown_time_absent_imported_at", "unknown_time_null_imported_at"):
                assert probe["response"]["status"] == (200 if current else 503)
                if current:
                    saved = probe["saved"]["detail"]["body"]
                    assert saved["timestamp"] is None and saved["timestamp_basis"] == "import"
                    assert probe["import_start_ms"] <= saved["imported_at"] <= probe["import_end_ms"]
        elif claim in ("structured_secrets", "spaced_bearer"):
            control = case in ("string_control", "one_space_control")
            assert probe["secret_retained"] == (not current and not control), (label, case)
        elif claim == "out_of_range_usage" and case in ("above_i64_input", "above_i64_output", "u64_max_input"):
            if probe["path"] == "import":
                assert probe["response"]["status"] == (400 if current else 200)
            else:
                field = "output_tokens" if case == "above_i64_output" else "input_tokens"
                saved = probe["saved"]
                assert saved["summary"][field] is None
                assert saved["summary"]["cost_known_requests"] == (0 if current else 1)
                assert (saved["detail"]["body"][field] is None) == current
                assert (saved["detail"]["body"]["cost_usd"] is None) == current

for index, result in enumerate(read("storage/groups-results.json")):
    assert result["binary_sha256"] == hashes[index]
    assert result["request_order_matches_newest_100"]
    assert result["answer_order_matches_newest_100"] == bool(index)
    assert result["untimed_request_timestamp"] is None
    if index:
        assert result["untimed_group_answer_timestamp"] is None
        assert result["untimed_group_answer_imported_at"] == result["untimed_request_imported_at"]
    else:
        assert result["untimed_group_answer_timestamp"] == result["untimed_request_imported_at"]
        assert result["untimed_group_answer_imported_at"] is None

for index, result in enumerate(read("storage/http-races-results.json")):
    assert result["sha256"] == hashes[index]
    assert len(result["attempts"]) == 54
    for operation, failures in (("request", 10), ("export-jsonl", 14), ("export-csv", 12)):
        attempts = [a for a in result["attempts"] if a["operation"] == operation]
        assert len(attempts) == 18
        assert sum(a["outcome"] == "torn_record" for a in attempts) == (0 if index else failures)
        if index:
            assert all(a["outcome"] == "consistent" for a in attempts)

snapshots = read("storage/snapshots-results.json")["results"]
assert len(snapshots) == 12
for result in snapshots:
    assert result["hook_consumed"] and result["completed_read_checkpoint_busy"] == 0
    fixed = result["version"] == "working"
    if result["operation"] == "export-csv":
        assert result["observed_answer_count"] == ('"0"' if not fixed and result["mutation"] == "delete" else '"1"')
    else:
        assert result["observed_parent_generation"] == "A"
        assert result["observed_answer_generations"] == (["A"] if fixed else [] if result["mutation"] == "delete" else ["B"])
        assert result["observed_labels"] == (["correct"] if fixed else [] if result["mutation"] == "delete" else ["incorrect"])

runpy.run_path(str(ROOT / "ui/verify-results.py"))
audit = read("test-quality/public-snapshot-regression/results.json")
assert audit["workspace_source_unchanged"] and audit["release_binary_unchanged"]
expected_failures = {"current": 0, "without_request_transaction": 2,
                     "without_export_transaction": 3, "without_both_transactions": 5}
assert {r["variant"] for r in audit["results"]} == set(expected_failures)
for result in audit["results"]:
    failed = expected_failures[result["variant"]]
    assert (result["passed"], result["failed"]) == (5 - failed, failed)
    assert result["test_exit_code"] == (101 if failed else 0)
    assert len(result["failed_tests"]) == failed
    log = (ROOT / "test-quality/public-snapshot-regression" / (result["variant"] + ".test.log")).read_text()
    assert f"{5 - failed} passed; {failed} failed;" in log
print("The subsequent public-regression mutation audit detects each removed transaction wrapper.")
print("Archived hashes and reported backend/storage observations match the retained evidence.")
