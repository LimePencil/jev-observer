#!/usr/bin/env python3
"""Unmodified binaries, only HTTP import/read/delete, scheduler-dependent race."""
import concurrent.futures
import csv
import hashlib
import io
import json
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from check_groups import BASELINE, CURRENT, OUT, ROOT, record


def run(binary):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="observer-http-read-race-") as directory:
        with (OUT / (binary.name + ".races.log")).open("w") as output:
            process = subprocess.Popen([str(binary), "--db", directory + "/history.sqlite", "--port", str(port)],
                                       stdout=output, stderr=subprocess.STDOUT)
            def api(path, data=None, method=None):
                req = urllib.request.Request(f"http://127.0.0.1:{port}" + path,
                    data=None if data is None else json.dumps(data).encode(), method=method,
                    headers={"Content-Type": "application/json", "x-observer-request": "1"})
                try:
                    with urllib.request.urlopen(req, timeout=20) as response:
                        return response.status, response.read().decode()
                except urllib.error.HTTPError as error:
                    return error.code, error.read().decode()
            try:
                for _ in range(200):
                    try:
                        api("/api/health")
                        break
                    except OSError:
                        if process.poll() is not None:
                            raise RuntimeError("Server exited")
                        time.sleep(0.025)
                results = []
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
                    for operation, path in (("request", "/api/requests/fresh-tie-000"),
                                            ("export-jsonl", "/api/export?window=all&format=jsonl"),
                                            ("export-csv", "/api/export?window=all&format=csv")):
                        for delay in [0, 0.0001, 0.0005, 0.001, 0.002, 0.005]:
                            for iteration in range(3):
                                imported = record(0, int(time.time() * 1000))
                                imported["request_extra"] = {"independent_padding": "x" * (2 * 1024 * 1024)}
                                imported["labels"] = [{"key": "flag", "label": "correct", "source": "independent"}]
                                status, body = api("/api/import", {"format": "observer-jsonl", "text": json.dumps(imported)})
                                assert status == 200 and json.loads(body)["imported"] == 1, (status, body)
                                future = workers.submit(api, path)
                                time.sleep(delay)
                                delete_status, _ = api("/api/data", method="DELETE")
                                status, body = future.result()
                                assert delete_status == 200
                                if status == 404:
                                    outcome = "record_deleted_before_parent_read"
                                    count = None
                                elif status != 200:
                                    outcome = "unexpected_http_status"
                                    count = None
                                elif operation == "export-csv":
                                    rows = list(csv.DictReader(io.StringIO(body)))
                                    count = int(rows[0]["answer_count"]) if rows else None
                                    outcome = "empty_export" if not rows else "consistent" if count == 1 else "torn_record"
                                elif not body.strip():
                                    count, outcome = None, "empty_export"
                                else:
                                    parsed = json.loads(body)
                                    count = len(parsed["answers"])
                                    outcome = "consistent" if count == 1 and len(parsed["labels"]) == 1 else "torn_record"
                                result = {"operation": operation, "delay_s": delay, "iteration": iteration,
                                          "status": status, "answer_count": count, "outcome": outcome}
                                if outcome == "torn_record":
                                    if operation != "export-csv":
                                        parsed["request_extra"]["independent_padding"] = "[2 MiB padding omitted from evidence]"
                                        result["observed_record"] = parsed
                                    else:
                                        result["observed_csv"] = body
                                results.append(result)
                counts = {operation: {outcome: sum(r["operation"] == operation and r["outcome"] == outcome for r in results)
                                      for outcome in sorted({r["outcome"] for r in results})}
                          for operation in ("request", "export-jsonl", "export-csv")}
                return {"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                        "evidence": "Unmodified binary, only product HTTP endpoints on isolated temporary database. Large valid 2 MiB metadata widens scheduler-dependent parent-read window.",
                        "summary": counts, "attempts": results}
            finally:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


results = []
for binary in (BASELINE, CURRENT):
    result = run(binary)
    results.append(result)
    print(json.dumps({k: v for k, v in result.items() if k != "attempts"}, indent=2), flush=True)
(OUT / "http-races-results.json").write_text(json.dumps(results, indent=2) + "\n")
