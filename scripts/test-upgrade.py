#!/usr/bin/env python3
"""Published 0.1.0 to candidate upgrade, rollback and backup checks; loopback only."""
import argparse
import base64
import csv
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import io
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
KEY = "upgrade-fixture-provider-placeholder"
REQUEST = json.loads((ROOT / "fixtures/sdk/request.json").read_text())
RESPONSE = (ROOT / "fixtures/sdk/response.json").read_bytes()


class Mock(BaseHTTPRequestHandler):
    calls = 0

    def do_POST(self):
        if self.path != "/v1/systemone" or self.headers.get("Authorization") != "Bearer " + KEY:
            self.send_error(403)
            return
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            while True:
                size = int(self.rfile.readline().split(b";", 1)[0], 16)
                if size == 0:
                    while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                        pass
                    break
                self.rfile.read(size)
                self.rfile.read(2)
        else:
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
        Mock.calls += 1
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(RESPONSE)))
        self.end_headers()
        self.wfile.write(RESPONSE)

    def log_message(self, *_):
        pass


class Upgrade:
    def __init__(self, args):
        self.args = args
        self.environment = {name: value for name, value in os.environ.items()
                            if name.lower() not in {"http_proxy", "https_proxy", "all_proxy", "typesafe_api_key"}}
        self.environment["NO_PROXY"] = "127.0.0.1,localhost"
        self.environment["JEV_OBSERVER_DB_KEY"] = secrets.token_hex(32)
        self.report = {"checked_at": datetime.now(timezone.utc).isoformat(),
                       "scope": "Synthetic loopback upgrade; dummy database approval hash, no native credential-store persistence or provider calls",
                       "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                       "credential_limit": "Does not establish native keychain upgrade/downgrade compatibility. Re-register a provider key after downgrading if a saved credential was rotated in 0.2.0.",
                       "checks": {}, "passed": False}
        self.process = None
        self.log = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        self.phase = "setup"

    def check(self, value, name):
        self.report["checks"][name] = bool(value)
        if not value:
            raise AssertionError(name)

    def request(self, route, method="GET", body=None, headers=None):
        auth = "Basic " + base64.b64encode(("observer:" + self.token).encode()).decode()
        supplied = headers or {"Authorization": auth, "X-Observer-Request": "1"}
        supplied = {**supplied, "Accept-Encoding": "identity"}
        if body is not None:
            supplied["Content-Type"] = "application/json"
            body = json.dumps(body).encode()
        request = urllib.request.Request(self.origin + route, body, supplied, method=method)
        with self.opener.open(request, timeout=getattr(self.args, "timeout", 10)) as response:
            return response.read()

    def api(self, route, method="GET", body=None, headers=None):
        return json.loads(self.request(route, method, body, headers))

    def start(self, binary, extra=()):
        self.stop()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        self.origin = f"http://127.0.0.1:{port}"
        self.log = (self.directory / "observer.log").open("ab")
        self.process = subprocess.Popen(
            [str(binary), "--db", str(self.database), "--port", str(port), "--upstream", self.upstream, *extra],
            env=self.environment, cwd=self.directory, stdin=subprocess.DEVNULL, stdout=self.log, stderr=self.log,
            creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
        )
        for _ in range(300):
            self.check(self.process.poll() is None, self.phase + "_starts")
            try:
                self.token = self.database.with_suffix(".access-token").read_text().strip()
                self.api("/api/health")
                return
            except (OSError, ValueError, urllib.error.URLError):
                time.sleep(0.1)
        raise AssertionError(self.phase + "_startup_deadline")

    def stop(self):
        if self.process:
            if self.process.poll() is None:
                self.process.send_signal(signal.CTRL_BREAK_EVENT if os.name == "nt" else signal.SIGTERM)
                try:
                    self.process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
            code = self.process.returncode
            self.process = None
            if self.log:
                self.log.close()
                self.log = None
            self.check(code == 0, self.phase + "_clean_shutdown")

    def probe(self, mode, path=None):
        result = subprocess.run([str(self.args.probe), str(path or self.database), mode], env=self.environment, capture_output=True, timeout=15)
        self.check(result.returncode == 0, "probe_" + mode)
        return json.loads(result.stdout) if mode == "show" else None

    def expect_rejected(self, path, name, key=None):
        before = hashlib.sha256(path.read_bytes()).hexdigest()
        environment = {**self.environment}
        if key:
            environment["JEV_OBSERVER_DB_KEY"] = key
        process = subprocess.Popen([str(self.args.candidate), "--db", str(path), "--port", "0", "--upstream", self.upstream],
                                   env=environment, cwd=self.directory, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            _, stderr = process.communicate(timeout=15)
            code = process.returncode
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise AssertionError(name + "_was_accepted") from None
        self.check(code != 0, name + "_rejected")
        category = b"Unsupported database schema" if name.startswith("future_") else b"file is not a database"
        self.check(category in stderr, name + "_expected_rejection_category")
        self.check(hashlib.sha256(path.read_bytes()).hexdigest() == before, name + "_database_bytes_unchanged")

    def snapshot(self):
        jsonl = [json.loads(line) for line in self.request("/api/export?window=all&format=jsonl").splitlines() if line.strip()]
        for record in jsonl:
            # 0.2 adds a read-time outcome projection, not a stored-data change.
            if "failed" in record:
                expected = (record.get("status") or 0) >= 400 or record.get("transport_error") is not None
                self.check(record.pop("failed") == expected, self.phase + "_derived_failure_projection")
        csv_rows = list(csv.DictReader(io.StringIO(self.request("/api/export?window=all&format=csv").decode())))
        return {"jsonl": jsonl, "csv": csv_rows}

    def run(self):
        for name in ("baseline", "candidate", "probe"):
            setattr(self.args, name, getattr(self.args, name).resolve(strict=True))
        self.report["probe_sha256"] = hashlib.sha256(self.args.probe.read_bytes()).hexdigest()
        for name in ("baseline", "candidate"):
            binary = getattr(self.args, name)
            result = subprocess.run([str(binary), "--version"], capture_output=True, env=self.environment, timeout=10, check=True)
            self.report[name] = {"version": result.stdout.decode().strip(), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
        self.check(self.report["baseline"]["version"] == "jev-observer 0.1.0", "published_baseline_version")
        server = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.upstream = f"http://127.0.0.1:{server.server_port}/v1/systemone"
        try:
            with tempfile.TemporaryDirectory(prefix="observer-upgrade-") as temporary:
                self.directory = Path(temporary) / ".jev-observer"
                self.directory.mkdir(mode=0o700)
                self.database = self.directory / "history.sqlite"
                try:
                    self.phase = "baseline"
                    self.start(self.args.baseline)
                    credential = self.api("/api/credentials", "PUT", {"api_key": KEY, "persist": False})
                    self.api("/v1/systemone", "POST", REQUEST, {"Authorization": "Bearer " + credential["client_token"], "X-Observer-Source": "upgrade-fixture"})
                    for _ in range(200):
                        if self.api("/api/health")["persisted"] == 1:
                            break
                        time.sleep(0.05)
                    dashboard = self.api("/api/dashboard?window=all")
                    self.check(dashboard["summary"]["request_count"] == 1, "baseline_history_created")
                    record_id = dashboard["requests"][0]["id"]
                    self.api("/api/requests/" + record_id + "/label", "POST", {"key": "department", "label": "correct"})
                    before = self.snapshot()
                    exported_text = json.dumps(before)
                    self.check(KEY not in exported_text and credential["client_token"] not in exported_text, "baseline_export_excludes_credentials")
                    self.stop()
                    self.check(not self.database.read_bytes().startswith(b"SQLite format 3"), "baseline_history_encrypted")
                    self.probe("seed-approval")
                    approval = self.probe("show")
                    backup = {path.name: path.read_bytes() for path in self.directory.glob("history.sqlite*")}
                    self.phase = "candidate"
                    self.start(self.args.candidate)
                    self.check(self.snapshot() == before, "upgrade_preserves_history_labels_and_exports")
                    self.check(self.probe("show") == approval, "upgrade_preserves_credential_approval")
                    self.check(not self.api("/api/credentials")["configured"], "unavailable_system_credential_not_activated")
                    self.api("/v1/systemone", "POST", REQUEST, {"Authorization": "Bearer " + KEY, "X-Observer-Access": self.token, "X-Observer-Source": "upgrade-fixture"})
                    for _ in range(200):
                        if self.api("/api/health")["persisted"] == 1:
                            break
                        time.sleep(0.05)
                    self.check(self.api("/api/dashboard?window=all")["summary"]["request_count"] == 2, "candidate_can_append_to_upgraded_history")
                    upgraded = self.snapshot()
                    self.stop()
                    self.phase = "rollback"
                    self.start(self.args.baseline)
                    self.check(self.snapshot() == upgraded, "baseline_can_read_candidate_database_and_new_record")
                    self.stop()
                    self.check(self.probe("show") == {**approval, "requests": 2}, "rollback_preserves_credential_approval")
                    self.phase = "backup_restore"
                    for path in self.directory.glob("history.sqlite*"):
                        path.unlink()
                    for name, data in backup.items():
                        (self.directory / name).write_bytes(data)
                    self.start(self.args.candidate)
                    self.check(self.snapshot() == before, "restored_backup_retains_history_labels_and_exports")
                    self.stop()
                    self.check(self.probe("show") == approval, "restored_backup_retains_credential_approval")
                    self.phase = "rejection"
                    self.expect_rejected(self.database, "wrong_key", secrets.token_hex(32))
                    future = self.directory / "future.sqlite"
                    shutil.copyfile(self.database, future)
                    self.probe("future-schema", future)
                    self.expect_rejected(future, "future_encrypted_schema")
                    future_plain = self.directory / "future-plain.sqlite"
                    self.probe("future-plaintext", future_plain)
                    self.expect_rejected(future_plain, "future_plaintext_schema")
                    self.check(Mock.calls == 2, "two_loopback_inferences_no_remote_calls")
                finally:
                    self.stop()
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    check = Upgrade(args)
    try:
        check.run()
        check.report["passed"] = True
    except Exception as error:
        check.report["failure"] = {"phase": check.phase, "type": type(error).__name__}
        if isinstance(error, AssertionError):
            check.report["failure"]["check"] = str(error)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(check.report, indent=2) + "\n")
    print(json.dumps(check.report))
    return 0 if check.report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
