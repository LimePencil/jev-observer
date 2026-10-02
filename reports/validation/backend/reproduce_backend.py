#!/usr/bin/env python3
"""Independent HTTP-only regression probes, with isolated processes and local mock.

No product/test imports. All request data, temporary databases and dummy credentials
are created here. Existing author tests are never invoked.
"""
import copy
import base64
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / ".jev-observer/independent-validation-rerun/backend"
OUT.mkdir(parents=True, exist_ok=True)
LIMIT = 8 * 1024 * 1024
MAX_I64 = 2**63 - 1
MAX_U64 = 2**64 - 1
NOW = int(time.time() * 1000)
BINARIES = {
    "baseline": Path(os.environ.get("JEV_VALIDATION_BASELINE", ROOT / ".jev-observer/before-refresh-observer")).resolve(),
    "current": Path(os.environ.get("JEV_VALIDATION_CURRENT", ROOT / "target/release/jev-observer")).resolve(),
}
MOCK_CASES = {}
MOCK_LOG = []


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


class Mock(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers["Content-Length"]))
        body = json.loads(raw)
        case = MOCK_CASES[body["probe_case"]]
        authorization = self.headers.get("Authorization")
        response = {
            "model": "local-mock-model",
            "answers": {"eligible": {"type": "noul", "noul": 0.8}},
            "usage": case.get("usage", {"input_tokens": 123, "output_tokens": 9}),
            "debug_echo": case.get("echo", "ordinary-public-text"),
        }
        if case.get("echo_authorization"):
            response["debug_echo"] = "Bearer " + authorization.split(None, 1)[1].strip()
        MOCK_LOG.append({"case": body["probe_case"], "authorization_received": authorization,
                         "upstream_response": response})
        data = compact(response)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


dashboard_authorization = ""
observer_access = ""


def request(port, method, path, data=None, headers=None):
    headers = dict(headers or {})
    if path.startswith("/api/") and dashboard_authorization:
        headers.setdefault("Authorization", dashboard_authorization)
    if path == "/v1/systemone" and observer_access:
        headers.setdefault("X-Observer-Access", observer_access)
    if data is not None:
        headers.setdefault("Content-Type", "application/json")
    if method != "GET" and path.startswith("/api/"):
        headers["X-Observer-Request"] = "1"
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=45)
    try:
        try:
            connection.request(method, path, data, headers)
        except BrokenPipeError:
            # A server may answer 413 and close before consuming a long upload.
            pass
        response = connection.getresponse()
        raw = response.read()
        try:
            parsed = json.loads(raw)
        except (ValueError, UnicodeError):
            parsed = raw.decode(errors="replace")
        return {"status": response.status, "body": parsed}
    finally:
        connection.close()


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def record(case):
    return {
        "schema_version": 1, "id": case, "timestamp": NOW,
        "source": case, "provider": "typesafe", "model": "local-mock-model",
        "requested_model": "local-mock-model", "status": 200, "duration_ms": 5,
        "input_tokens": 123, "output_tokens": 9, "cost_usd": None,
        "cost_basis": None, "source_event_id": case, "import_format": None,
        "capture_complete": True, "state_retained": False, "state": None,
        "transport_error": None, "sample": False,
        "answers": [{"key": "eligible", "definition": {"type": "noul", "instructions": "Is eligible?"},
                     "raw_answer": {"type": "noul", "noul": 0.8}}],
        "actions": [], "labels": [],
    }


def imported(port, item):
    return request(port, "POST", "/api/import", compact({"format": "observer-jsonl", "text": compact(item).decode()}))


def observation(port, source, wait=False):
    path = "/api/dashboard?source=" + urllib.parse.quote(source, safe="")
    deadline = time.monotonic() + (8 if wait else 0)
    while True:
        dashboard = request(port, "GET", path)
        rows = dashboard.get("body", {}).get("requests", []) if isinstance(dashboard.get("body"), dict) else []
        if rows or time.monotonic() >= deadline:
            break
        time.sleep(0.03)
    result = {"dashboard_status": dashboard["status"], "request_count": len(rows),
              "summary": dashboard.get("body", {}).get("summary")}
    if rows:
        result["row"] = rows[0]
        result["detail"] = request(port, "GET", "/api/requests/" + urllib.parse.quote(rows[0]["id"], safe=""))
    return result


def run_probe_suite(label, executable, upstream_port):
    global dashboard_authorization, observer_access
    dashboard_authorization = ""
    observer_access = ""
    suite = {"binary": str(executable), "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(), "probes": []}
    with tempfile.TemporaryDirectory(prefix="jev-independent-backend-") as temporary:
        port = free_port()
        log_path = OUT / f"{label}-server.log"
        env = {k: v for k, v in os.environ.items() if k != "TYPESAFE_API_KEY"}
        env["JEV_OBSERVER_DB_KEY"] = os.urandom(32).hex()
        command = [str(executable), "--port", str(port), "--db", str(Path(temporary) / "probe.sqlite"),
                   "--upstream", f"http://127.0.0.1:{upstream_port}/v1/systemone",
                   "--input-price-per-million", "1.25", "--output-price-per-million", "2.5",
                   "--redact-key", "private_contact"]
        suite["command"] = command
        token_path = Path(temporary) / "probe.access-token"
        with log_path.open("w") as log:
            process = subprocess.Popen(command, stdout=log, stderr=log, env=env)
            try:
                deadline = time.monotonic() + 12
                while True:
                    try:
                        if token_path.exists():
                            token = token_path.read_text()
                            observer_access = token
                            dashboard_authorization = "Basic " + base64.b64encode(f"observer:{token}".encode()).decode()
                        if request(port, "GET", "/api/health")["status"] == 200:
                            break
                    except OSError:
                        pass
                    if process.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError(f"Cannot start {label}: {log_path.read_text()}")
                    time.sleep(0.04)

                # A valid one-record file with actual retained plain or quote-heavy text.
                for name, size, quote_heavy in [
                    ("small_control", 2048, False),
                    ("below_8m_quote_heavy", 7 * 1024 * 1024, True),
                    ("exact_8m_plain", LIMIT, False),
                    ("above_8m_plain", LIMIT + 1, False),
                ]:
                    item = record("size_" + name)
                    item["response_extra"] = {"document": ""}
                    empty_size = len(compact(item))
                    remaining = size - empty_size
                    # JSON source renders one quote as two bytes; wrapper escapes both.
                    item["response_extra"]["document"] = ('"' * (remaining // 2) + "x" * (remaining % 2)) if quote_heavy else "x" * remaining
                    text_bytes = compact(item)
                    assert len(text_bytes) == size
                    json.loads(text_bytes)
                    envelope = compact({"format": "observer-jsonl", "text": text_bytes.decode()})
                    response = request(port, "POST", "/api/import", envelope)
                    evidence = {"claim": "import_envelope", "case": name, "decoded_text_bytes": len(text_bytes),
                                "transport_bytes": len(envelope), "valid_json_record": True, "response": response}
                    # Avoid duplicating megabytes; fetch summary only for success verification.
                    if response["status"] == 200:
                        dash = request(port, "GET", "/api/dashboard?source=" + item["source"])
                        evidence["stored_request_count"] = len(dash["body"].get("requests", []))
                    suite["probes"].append(evidence)

                # Ordinary archive shape: 5,000 small records (well below 10,000),
                # each with a short retained note, totaling 64 KiB below 8 MiB.
                archive_source = "size_many_small_records"
                archive_lines = []
                archive_target = LIMIT - 65536
                for number in range(5000):
                    item = record(f"archive-record-{number:05d}")
                    item["source"] = archive_source
                    item["response_extra"] = {"note": ""}
                    record_bytes = (archive_target // 5000) - 1
                    item["response_extra"]["note"] = "A" * (record_bytes - len(compact(item)))
                    archive_lines.append(compact(item))
                archive_text = b"\n".join(archive_lines) + b"\n"
                envelope = compact({"format": "observer-jsonl", "text": archive_text.decode()})
                response = request(port, "POST", "/api/import", envelope)
                dash = request(port, "GET", "/api/dashboard?source=" + archive_source)
                suite["probes"].append({"claim": "import_envelope", "case": "5000_small_records_below_8m",
                    "decoded_text_bytes": len(archive_text), "transport_bytes": len(envelope),
                    "record_count": 5000, "valid_json_records": True, "response": response,
                    "dashboard_summary": dash["body"].get("summary"), "dashboard_status": dash["status"]})

                for name, timestamp, imported_at in [
                    ("known_time_control", NOW, "ABSENT"),
                    ("omitted_timestamp_field", "ABSENT", "ABSENT"),
                    ("unknown_time_absent_imported_at", None, "ABSENT"),
                    ("unknown_time_null_imported_at", None, None),
                    ("unknown_time_valid_imported_at_control", None, NOW - 1000),
                    ("invalid_imported_at_type", None, "last week"),
                ]:
                    item = record("timestamp_" + name)
                    if timestamp == "ABSENT":
                        del item["timestamp"]
                    else:
                        item["timestamp"] = timestamp
                    if imported_at != "ABSENT":
                        item["imported_at"] = imported_at
                    start = int(time.time() * 1000)
                    response = imported(port, item)
                    end = int(time.time() * 1000)
                    evidence = {"claim": "unknown_timestamp", "case": name, "input": item,
                                "import_start_ms": start, "import_end_ms": end, "response": response}
                    evidence["saved"] = observation(port, item["source"])
                    suite["probes"].append(evidence)

                secret_specs = [
                    ("string_control", {"api_key": "dummy-key-scalar-G4K9"}, "dummy-key-scalar-G4K9"),
                    ("array", {"api_key": ["dummy-key-array-P9W4"]}, "dummy-key-array-P9W4"),
                    ("object", {"client_secret": {"active": "dummy-key-object-A8H2"}}, "dummy-key-object-A8H2"),
                    ("headers_array", {"request_headers": {"Authorization": ["Bearer dummy-header-H7M3"]}}, "dummy-header-H7M3"),
                    ("custom_nested_object", {"private_contact": {"emails": ["dummy-contact@example.test"]}}, "dummy-contact@example.test"),
                    ("custom_in_unretained_state", {"state": {"private_contact": {"emails": ["dummy-state-contact@example.test"]}}}, "dummy-state-contact@example.test"),
                ]
                for name, sensitive, secret in secret_specs:
                    case = "secret_" + name
                    MOCK_CASES[case] = {"echo": "Echo observed: " + secret}
                    payload = {"probe_case": case, "model": "local-mock-model",
                               "questions": {"eligible": {"type": "noul", "instructions": "Is eligible?"}}, **sensitive}
                    response = request(port, "POST", "/v1/systemone", compact(payload),
                                       {"Authorization": "Bearer dummy-forwarding-key-T2N8", "X-Observer-Source": case})
                    saved = observation(port, case, True)
                    suite["probes"].append({"claim": "structured_secrets", "case": name, "path": "live",
                                            "request": payload, "proxy_response": response, "saved": saved,
                                            "secret_retained": secret in json.dumps(saved)})
                    item = record("import_" + case)
                    if "state" in sensitive:
                        item["state_retained"] = True
                        item["state"] = copy.deepcopy(sensitive["state"])
                    else:
                        item["request_extra"] = copy.deepcopy(sensitive)
                    item["response_extra"] = {"debug_echo": "Echo observed: " + secret}
                    response = imported(port, item)
                    saved = observation(port, item["source"])
                    suite["probes"].append({"claim": "structured_secrets", "case": name, "path": "import",
                                            "import_input": item, "response": response, "saved": saved,
                                            "secret_retained": secret in json.dumps(saved)})

                for name, prefix in [("one_space_control", "Bearer "), ("four_spaces", "Bearer    "), ("mixed_case_spaces", "bEaReR   ")]:
                    case = "bearer_" + name
                    secret = "dummy-bearer-token-U6Q1"
                    MOCK_CASES[case] = {"echo_authorization": True}
                    payload = {"probe_case": case, "model": "local-mock-model",
                               "questions": {"eligible": {"type": "noul", "instructions": "Is eligible?"}}}
                    response = request(port, "POST", "/v1/systemone", compact(payload),
                                       {"Authorization": prefix + secret, "X-Observer-Source": case})
                    saved = observation(port, case, True)
                    suite["probes"].append({"claim": "spaced_bearer", "case": name, "sent_authorization": prefix + secret,
                                            "proxy_response": response, "saved": saved, "secret_retained": secret in json.dumps(saved)})

                for name, field, count in [
                    ("normal_input", "input_tokens", 432),
                    ("zero_input", "input_tokens", 0),
                    ("i64_max_input", "input_tokens", MAX_I64),
                    ("above_i64_input", "input_tokens", MAX_I64 + 1),
                    ("above_i64_output", "output_tokens", MAX_I64 + 1),
                    ("u64_max_input", "input_tokens", MAX_U64),
                    ("negative_input_control", "input_tokens", -1),
                    ("fractional_input_control", "input_tokens", 1.25),
                ]:
                    case = "tokens_" + name
                    usage = {"input_tokens": 10, "output_tokens": 4, field: count}
                    MOCK_CASES[case] = {"usage": usage}
                    payload = {"probe_case": case, "model": "local-mock-model",
                               "questions": {"eligible": {"type": "noul", "instructions": "Is eligible?"}}}
                    response = request(port, "POST", "/v1/systemone", compact(payload),
                                       {"Authorization": "Bearer dummy-forwarding-key-T2N8", "X-Observer-Source": case})
                    saved = observation(port, case, True)
                    suite["probes"].append({"claim": "out_of_range_usage", "case": name, "path": "live",
                                            "upstream_usage": usage, "proxy_response": response, "saved": saved})
                    if name in ("normal_input", "i64_max_input", "above_i64_input", "above_i64_output"):
                        item = record("import_" + case)
                        item[field] = count
                        imported_response = imported(port, item)
                        suite["probes"].append({"claim": "out_of_range_usage", "case": name, "path": "import",
                                                "count": count, "field": field, "response": imported_response,
                                                "saved": observation(port, item["source"])})
            finally:
                process.send_signal(signal.SIGTERM)
                try:
                    suite["shutdown_returncode"] = process.wait(timeout=18)
                except subprocess.TimeoutExpired:
                    process.kill()
                    suite["shutdown_returncode"] = process.wait()
    suite["sha256_after"] = hashlib.sha256(executable.read_bytes()).hexdigest()
    return suite


def main():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Mock)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report = {"created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "method": "fresh loopback HTTP probes; temporary DB per binary; generated dummy secrets only",
              "suites": {}}
    try:
        for label, executable in BINARIES.items():
            report["suites"][label] = run_probe_suite(label, executable, server.server_port)
            (OUT / "evidence.json").write_text(json.dumps(report, indent=2) + "\n")
            print(label + ": " + str(len(report["suites"][label]["probes"])) + " probes complete", flush=True)
    finally:
        server.shutdown()
        server.server_close()
        report["mock_requests"] = MOCK_LOG
        (OUT / "evidence.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
