#!/usr/bin/env python3
"""Real SDKs, temporary database, local mock: no paid inference or remote API calls."""
from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import importlib.metadata
import json
import math
import os
import secrets
from pathlib import Path
import platform
import socket
import subprocess
import signal
import tempfile
import threading
import time
import urllib.request
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx2
from typesafe_sdk import Choice, Noul, Score, RetryPolicy, TypeSafeAPIError, TypeSafeClient

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "fixtures" / "sdk"
KEY = "sdk-compat-placeholder"
REQUEST = json.loads((FIXTURES / "request.json").read_text())
SUCCESS_BYTES = (FIXTURES / "response.json").read_bytes()
ERROR_BYTES = (FIXTURES / "error.json").read_bytes()
ERROR_BODY = json.loads(ERROR_BYTES)


class Mock(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    calls: list[dict] = []
    failures: list[str] = []

    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            assert self.path == "/v1/systemone", self.path
            assert self.headers.get("Authorization") == f"Bearer {KEY}"
            assert self.headers.get("Accept-Encoding") == "identity", "Identity encoding header did not reach upstream"
            assert not any(name.lower().startswith("x-observer-") for name in self.headers)
            if "chunked" in self.headers.get("Transfer-Encoding", "").lower():
                chunks = []
                while True:
                    size = int(self.rfile.readline().split(b";", 1)[0], 16)
                    if size == 0:
                        while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                            pass
                        break
                    chunks.append(self.rfile.read(size))
                    assert self.rfile.read(2) == b"\r\n"
                body = b"".join(chunks)
            else:
                body = self.rfile.read(int(self.headers["Content-Length"]))
            payload = json.loads(body)
            assert payload["request_extension"] == REQUEST["request_extension"]
            assert len(payload["questions"]) == 3
            self.calls.append(payload)
            failed = payload.get("state", {}).get("fixture_error") is True
            response = ERROR_BYTES if failed else SUCCESS_BYTES
            self.send_response(422 if failed else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.send_header("X-SDK-Fixture", "preserved")
            self.send_header("X-TypeSafe-Request-ID", "synthetic-sdk-request")
            self.end_headers()
            self.wfile.write(response)
        except Exception as error:
            self.failures.append(repr(error))
            self.close_connection = True


class GuardedTransport(httpx2.HTTPTransport):
    def __init__(self, origin: str):
        super().__init__(trust_env=False, retries=0)
        self.origin = origin
        self.attempts = 0

    def handle_request(self, request):
        assert str(request.url) == self.origin + "/v1/systemone", "External SDK request blocked"
        self.attempts += 1
        response = super().handle_request(request)
        response.read()
        assert response.content == (SUCCESS_BYTES if response.status_code == 200 else ERROR_BYTES), "Proxy changed response bytes"
        assert response.headers["x-sdk-fixture"] == "preserved"
        return response


def python_sdk(origin: str, access_token: str, api_key: str = KEY) -> dict:
    version = importlib.metadata.version("typesafe-sdk")
    assert version == "0.7.1", "Install the pinned Python requirements"
    transport = GuardedTransport(origin)
    with TypeSafeClient(
        api_key=api_key, base_url=origin, model=REQUEST["model"],
        retry=RetryPolicy(max_retries=0), timeout=5.0,
        headers={"x-observer-source": "sdk-python", "x-observer-access": access_token, "Accept-Encoding": "identity"}, transport=transport,
    ) as client:
        for index in range(3):
            definition = copy.deepcopy(REQUEST["questions"])
            if index == 2:
                definition["department"]["instructions"] += " Treat outages as technical."
            questions = {
                "department": Choice(instructions=definition["department"]["instructions"], criteria=definition["department"]["criteria"]),
                "urgency": Noul(instructions=definition["urgency"]["instructions"]),
                "frustration": Score(instructions=definition["frustration"]["instructions"], criteria=definition["frustration"]["criteria"]),
            }
            response = client.system_one(
                state={**REQUEST["state"], "sequence": index}, questions=questions,
                extra_body={"request_extension": REQUEST["request_extension"]},
            )
            assert response.choices["department"].choice == "billing"
            assert response.nouls["urgency"].noul == 0.9
            assert response.scores["frustration"].score == 1.6
            assert response.usage.input_tokens == 100
        try:
            client.system_one(
                state={"fixture_error": True}, questions=REQUEST["questions"],
                extra_body={"request_extension": REQUEST["request_extension"]},
            )
        except TypeSafeAPIError as error:
            assert error.status == 422
            assert error.body == ERROR_BODY
        else:
            raise AssertionError("Expected SDK to surface upstream 422")
    assert transport.attempts == 4, "Unexpected retries"
    return {"sdk": "typesafe-sdk", "version": version, "python": platform.python_version(), "attempts": 4, "successes": 3, "errors": 1, "raw_bytes_preserved": True}


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def local_auth(token: str) -> str:
    return "Basic " + base64.b64encode(f"observer:{token}".encode()).decode()


def get(origin: str, route: str, access_token: str):
    # Explicit no-proxy opener ignores any caller HTTP_PROXY configuration.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    request = urllib.request.Request(origin + route, headers={"Authorization": local_auth(access_token)})
    with opener.open(request, timeout=5) as response:
        return json.load(response)


def register_session_key(origin: str, access_token: str) -> str:
    body = json.dumps({"api_key": KEY, "persist": False}).encode()
    request = urllib.request.Request(
        origin + "/api/credentials", body, method="PUT",
        headers={"Content-Type": "application/json", "X-Observer-Request": "1", "Authorization": local_auth(access_token)},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=5) as response:
        assert response.status == 200
        payload = json.load(response)
    assert payload["storage"] == "session"
    assert payload["client_token"].startswith("jo_local_")
    return payload["client_token"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/jev-observer")
    parser.add_argument("--node-sdk", type=Path, required=True, help="Installed @typesafe-ai/sdk package directory")
    parser.add_argument("--output", type=Path, default=FIXTURES / "last-result.json")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    assert args.node_sdk.resolve().is_dir()
    mock = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
    thread = threading.Thread(target=mock.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{free_port()}"
    env = {key: value for key, value in os.environ.items() if not key.startswith("TYPESAFE_") and key.lower() not in {"http_proxy", "https_proxy", "all_proxy"}}
    env["NO_PROXY"] = "127.0.0.1,localhost"
    env["TYPESAFE_API_KEY"] = KEY
    env["JEV_OBSERVER_DB_KEY"] = secrets.token_hex(32)
    process = None
    try:
        with tempfile.TemporaryDirectory(prefix="jev-sdk-compat-") as temporary:
            # Windows runner TEMP can be shared. Exercise the same private
            # workspace ACL setup as the default user installation.
            directory = Path(temporary) / ".jev-observer"
            directory.mkdir()
            log_path = directory / "proxy.log"
            access_path = directory / "history.access-token"
            with log_path.open("wb") as logfile:
                process = subprocess.Popen([
                    str(binary), "--port", origin.rsplit(":", 1)[1], "--db", str(Path(directory) / "history.sqlite"),
                    "--upstream", f"http://127.0.0.1:{mock.server_port}/v1/systemone",
                    "--input-price-per-million", "2", "--output-price-per-million", "1",
                ], env=env, stdout=logfile, stderr=subprocess.STDOUT,
                    creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0)
                for attempt in range(100):
                    if process.poll() is not None:
                        raise AssertionError(log_path.read_text())
                    try:
                        access_token = access_path.read_text()
                        get(origin, "/api/health", access_token)
                        break
                    except (OSError, ValueError):
                        time.sleep(0.05)
                else:
                    raise AssertionError("Proxy startup timed out")
                python_result = python_sdk(origin, access_token)
                node_env = {**env, "OBSERVER_SDK_PROXY": origin, "OBSERVER_SDK_PACKAGE": str(args.node_sdk.resolve()), "OBSERVER_SDK_FIXTURES": str(FIXTURES), "OBSERVER_SDK_ACCESS": access_token}
                node_run = subprocess.run(["node", str(ROOT / "scripts/sdk-compatibility.mjs")], env=node_env, text=True, capture_output=True, timeout=30)
                assert node_run.returncode == 0, node_run.stderr
                javascript_result = json.loads(node_run.stdout)
                local_token = register_session_key(origin, access_token)
                registered_python_result = python_sdk(origin, access_token, local_token)
                registered_node_run = subprocess.run(
                    ["node", str(ROOT / "scripts/sdk-compatibility.mjs")],
                    env={**node_env, "OBSERVER_SDK_KEY": local_token},
                    text=True, capture_output=True, timeout=30,
                )
                assert registered_node_run.returncode == 0, registered_node_run.stderr
                registered_javascript_result = json.loads(registered_node_run.stdout)
                assert not Mock.failures, Mock.failures
                assert len(Mock.calls) == 16, f"Expected 16 upstream attempts, received {len(Mock.calls)}"
                for _ in range(200):
                    health = get(origin, "/api/health", access_token)
                    if health["persisted"] == 16:
                        break
                    time.sleep(0.05)
                else:
                    raise AssertionError(f"Persistence did not drain: {health}")
                assert health["dropped"] == 0 and health["write_failures"] == 0 and health["truncated"] == 0
                dashboard = get(origin, "/api/dashboard?window=all", access_token)
                later_health = get(origin, "/api/health", access_token)
                assert isinstance(health["process_id"], str) and health["process_id"]
                assert health["process_id"] == dashboard["health"]["process_id"] == later_health["process_id"]
                assert health["sample_sequence"] < dashboard["health"]["sample_sequence"] < later_health["sample_sequence"]
                summary = dashboard["summary"]
                assert summary["request_count"] == 16, summary
                assert summary["answer_count"] == 48, summary
                assert summary["error_count"] == 4, summary
                assert summary["incomplete_count"] == 0, summary
                assert summary["input_tokens"] == 1200 and summary["output_tokens"] == 144, summary
                assert summary["cost_known_requests"] == 12, summary
                assert math.isclose(summary["cost_usd"], 0.002544), summary
                assert len(dashboard["groups"]) == 8, dashboard["groups"]
                assert sum(group["valid_count"] for group in dashboard["groups"]) == 36
                for source in ("sdk-python", "sdk-javascript"):
                    groups = [group for group in dashboard["groups"] if group["source"] == source]
                    assert sorted(group["valid_count"] for group in groups) == [2, 4, 6, 6]
                for row in dashboard["requests"]:
                    record = get(origin, "/api/requests/" + row["id"], access_token)
                    serialized = json.dumps(record)
                    assert KEY not in serialized, "Credential entered persistence"
                    assert local_token not in serialized, "Local token entered persistence"
                    assert "private-sdk-input" not in serialized, "Unrequested input retention"
                    assert record["request_extra"]["request_extension"] == REQUEST["request_extension"]
                    if row["status"] == 200:
                        assert record["response_extra"]["provider_extension"]["wire"] is True
                        assert record["response_extra"]["credential_echo"] == "[REDACTED]"
                        assert record["usage_extra"]["usage_extension"] == "preserved"
                        department = next(answer for answer in record["answers"] if answer["key"] == "department")
                        assert department["raw_answer"]["answer_extension"]["preserved"] is True
                        assert all(answer["valid"] for answer in record["answers"])
                    else:
                        assert all(not answer["valid"] for answer in record["answers"])
                        assert record["cost_usd"] is None
                report = {
                    "checked_at": datetime.now(timezone.utc).isoformat(), "scope": "Local mock only; no provider inference",
                    "binary_sha256": binary_hash, "platform": platform.platform(),
                    "python": python_result, "javascript": javascript_result,
                    "registered_python": registered_python_result,
                    "registered_javascript": registered_javascript_result,
                    "upstream_attempts": len(Mock.calls), "summary": summary,
                    "groups": len(dashboard["groups"]), "valid_answers": 36,
                    "request_extensions_preserved": True, "response_bytes_preserved": True,
                    "accept_encoding_identity_forwarded": True,
                    "health_endpoints_share_sample_order": True,
                    "credentials_and_state_excluded": True, "registered_token_excluded": True,
                    "capture_gaps": health["dropped"], "passed": True,
                }
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(report, indent=2) + "\n")
                print(json.dumps(report, indent=2))
                # Windows cannot remove a workspace while the process holds its
                # database and token files. Stop before TemporaryDirectory exits.
                if os.name == "nt":
                    process.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    process.terminate()
                process.wait(timeout=20)
                assert process.returncode == 0, "Proxy did not shut down cleanly"
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        mock.shutdown()
        mock.server_close()


if __name__ == "__main__":
    main()
