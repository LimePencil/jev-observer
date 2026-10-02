#!/usr/bin/env python3
"""Fresh black-box group reproduction; only HTTP import/read, separate temp DBs."""
import hashlib
import json
import os
import pathlib
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]
OUT = ROOT / ".jev-observer/independent-validation-rerun/storage"
OUT.mkdir(parents=True, exist_ok=True)

BASELINE = pathlib.Path(os.environ.get("JEV_VALIDATION_BASELINE", ROOT / ".jev-observer/before-refresh-observer")).resolve()
CURRENT = pathlib.Path(os.environ.get("JEV_VALIDATION_CURRENT", ROOT / "target/release/jev-observer")).resolve()


def record(index, timestamp):
    return {
        "schema_version": 1,
        "id": f"fresh-tie-{index:03}", "timestamp": timestamp,
        "source": "independent-storage-validation", "provider": "test",
        "model": "test", "requested_model": "test", "status": 200,
        "duration_ms": 1.0, "input_tokens": None, "output_tokens": None,
        "cost_usd": None, "cost_basis": None, "source_event_id": None,
        "import_format": None, "capture_complete": True,
        "state_retained": False, "state": None, "transport_error": None,
        "sample": False, "actions": [], "labels": [],
        "answers": [{"key": "flag", "definition": {"type": "noul", "instructions": "Flag this?"},
                     "raw_answer": {"type": "noul", "noul": 0.7}}],
    }


def run(binary):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="observer-storage-blackbox-") as directory:
        log = OUT / (binary.name + ".groups.log")
        with log.open("w") as output:
            process = subprocess.Popen([str(binary), "--db", directory + "/history.sqlite", "--port", str(port)],
                                       stdout=output, stderr=subprocess.STDOUT)
            def api(path, data=None):
                req = urllib.request.Request(f"http://127.0.0.1:{port}" + path,
                    data=None if data is None else json.dumps(data).encode(),
                    headers={"Content-Type": "application/json", "x-observer-request": "1"})
                with urllib.request.urlopen(req, timeout=20) as response:
                    return json.load(response)
            try:
                for _ in range(200):
                    try:
                        api("/api/dashboard?window=all")
                        break
                    except OSError:
                        if process.poll() is not None:
                            raise RuntimeError(log.read_text())
                        time.sleep(0.025)
                now = int(time.time() * 1000)
                records = [record(i, now) for i in range(105)]
                imported = api("/api/import", {"format": "observer-jsonl", "text": "\n".join(map(json.dumps, records))})
                dashboard = api("/api/dashboard?window=all")
                group_id = dashboard["groups"][0]["id"]
                detail = api(f"/api/groups/{group_id}?window=all")
                request_ids = [r["id"] for r in detail["requests"]]
                answer_ids = [a["request_id"] for a in detail["answers"]]
                expected = [f"fresh-tie-{i:03}" for i in range(104, 4, -1)]
                untimed = record(999, None)
                untimed["imported_at"] = now + 1000
                untimed["timestamp_basis"] = "import"
                second_import = api("/api/import", {"format": "observer-jsonl", "text": json.dumps(untimed)})
                untimed_detail = api(f"/api/groups/{group_id}?window=all")
                observation = next(a for a in untimed_detail["answers"] if a["request_id"] == untimed["id"])
                request = api("/api/requests/" + untimed["id"])
                return {
                    "binary": str(binary),
                    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                    "evidence": "Unmodified binary through HTTP; independently generated input; isolated temporary database",
                    "same_timestamp": now, "import_response": imported,
                    "request_order_matches_newest_100": request_ids == expected,
                    "answer_order_matches_newest_100": answer_ids == expected,
                    "request_ids": request_ids, "answer_ids": answer_ids,
                    "answers_omitted_newest_requests": sorted(set(request_ids) - set(answer_ids)),
                    "answers_included_older_requests": sorted(set(answer_ids) - set(request_ids)),
                    "untimed_import_response": second_import,
                    "untimed_request_timestamp": request["timestamp"],
                    "untimed_request_imported_at": request.get("imported_at"),
                    "untimed_group_answer_timestamp": observation["timestamp"],
                    "untimed_group_answer_imported_at": observation.get("imported_at"),
                }
            finally:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    results = [run(BASELINE), run(CURRENT)]
    (OUT / "groups-results.json").write_text(json.dumps(results, indent=2) + "\n")
    for result in results:
        print(json.dumps({k: v for k, v in result.items() if k not in ("request_ids", "answer_ids")}, indent=2))
