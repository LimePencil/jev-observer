#!/usr/bin/env python3
"""Opt-in live OpenRouter smoke check: five upstream calls using synthetic data.

Uses plain HTTP clients, not either official SDK. Never sources dotenv files,
prints credentials, retains raw provider responses, or saves the database key.
The temporary encrypted workspace and subprocess logs are removed on exit.
"""
from __future__ import annotations

import argparse
import base64
import copy
import csv
from datetime import datetime, timezone
import hashlib
import io
import json
import math
import os
from pathlib import Path
import platform
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

UPSTREAM = "https://openrouter.ai/api/v1/systemone"
MODEL = "jev-1.13"
SOURCE = "live-openrouter-smoke"
STATE_MARKER = "private-synthetic-live-input-9f814d"
REQUEST = {
    "model": MODEL,
    "state": {"ticket": STATE_MARKER + ": I was charged twice. Please refund the duplicate payment today."},
    "questions": {
        "department": {"type": "choice", "instructions": "Which team should handle this ticket?",
                       "criteria": {"billing": "Payments", "technical": "Bugs", "sales": "Plans"}},
        "urgency": {"type": "noul", "instructions": "Does the customer request a response today?"},
        "frustration": {"type": "score", "instructions": "How frustrated is the customer?",
                        "criteria": ["Calm", "Frustrated", "Very angry"]},
    },
}


class CheckFailed(Exception):
    """Contains only a harness-authored check name, never upstream text."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, target):
        return None


def read_key(name, env_file):
    if not env_file:
        value = os.environ.get(name, "")
    else:
        value = ""
        for line in env_file.read_text().splitlines():
            line = line.strip()
            if line.startswith("export "):
                line = line[7:].lstrip()
            key, separator, candidate = line.partition("=")
            if separator and key.strip() == name:
                candidate = candidate.strip()
                if candidate[:1] in ("'", '"'):
                    quote = candidate[0]
                    end = candidate.find(quote, 1)
                    tail = candidate[end + 1:].strip()
                    if end < 0 or (tail and not tail.startswith("#")):
                        raise CheckFailed("dotenv_literal_is_valid")
                    value = candidate[1:end]
                else:
                    value = candidate.split(" #", 1)[0].strip()
    if not value or any(character.isspace() for character in value):
        raise CheckFailed("provider_key_is_present")
    return value


def numeric(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


class Smoke:
    def __init__(self, args):
        self.args = args
        self.secrets = []
        self.process = None
        self.logfile = None
        self.phase = "configuration"
        self.access = ""
        self.origin = ""
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        self.report = {
            "checked_at": datetime.now(timezone.utc).isoformat(),
            "scope": "Live OpenRouter, synthetic data, plain HTTP client; no SDK compatibility claim",
            "upstream": UPSTREAM, "model": MODEL, "platform": platform.platform(),
            "checks": {}, "calls": [], "errors": [], "passed": False,
        }

    def check(self, condition, name, required=False):
        self.report["checks"][name] = bool(condition)
        if not condition and required:
            raise CheckFailed(name)
        return bool(condition)

    def scrub(self, value):
        if isinstance(value, dict):
            return {key: self.scrub(item) for key, item in value.items()
                    if key.lower() not in {"api_key", "client_token", "authorization", "access_token", "database_key"}}
        if isinstance(value, list):
            return [self.scrub(item) for item in value]
        if isinstance(value, str):
            for secret in self.secrets:
                value = value.replace(secret, "[REDACTED]")
            return value
        if isinstance(value, float) and not math.isfinite(value):
            return None
        return value

    def request(self, route, method="GET", body=None, headers=None, timeout=15):
        supplied = {"Accept-Encoding": "identity"}
        if headers is None:
            supplied["Authorization"] = "Basic " + base64.b64encode(("observer:" + self.access).encode()).decode()
            supplied["X-Observer-Request"] = "1"
        else:
            supplied.update(headers)
        if body is not None:
            supplied["Content-Type"] = "application/json"
            body = json.dumps(body).encode()
        request = urllib.request.Request(self.origin + route, body, supplied, method=method)
        try:
            response = self.opener.open(request, timeout=timeout)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, response.read(), dict(response.headers)

    def api(self, route, method="GET", body=None):
        status, raw, _ = self.request(route, method, body)
        self.check(status == 200, "api_" + method.lower() + "_" + route.split("?")[0], required=True)
        return json.loads(raw)

    def start(self, binary, directory, environment):
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        self.origin = f"http://127.0.0.1:{port}"
        self.logfile = (directory / "proxy.log").open("ab")
        self.process = subprocess.Popen(
            [str(binary), "--port", str(port), "--db", str(directory / "history.sqlite"), "--upstream", UPSTREAM],
            env=environment, cwd=directory, stdin=subprocess.DEVNULL,
            stdout=self.logfile, stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0,
        )
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            self.check(self.process.poll() is None, "observer_stays_running", required=True)
            try:
                self.access = (directory / "history.access-token").read_text().strip()
                if self.access not in self.secrets:
                    self.secrets.append(self.access)
                status, _, _ = self.request("/api/health")
                if status == 200:
                    return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.1)
        raise CheckFailed("observer_startup_deadline")

    def stop(self):
        if self.process is not None:
            if self.process.poll() is None:
                self.process.send_signal(signal.CTRL_BREAK_EVENT if os.name == "nt" else signal.SIGTERM)
                try:
                    self.process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
            self.check(self.process.returncode == 0, "clean_shutdown_" + self.phase)
            self.process = None
        if self.logfile is not None:
            self.logfile.close()
            self.logfile = None

    def inference(self, name, body, token, direct=False):
        headers = {"Authorization": "Bearer " + token, "X-Observer-Source": SOURCE}
        if direct:
            headers["X-Observer-Access"] = self.access
        before = time.monotonic()
        status, raw, response_headers = self.request("/v1/systemone", "POST", body, headers, timeout=120)
        try:
            response = json.loads(raw)
        except ValueError:
            response = {}
        if not isinstance(response, dict):
            response = {}
        usage = response.get("usage")
        usage = usage if isinstance(usage, dict) else {}
        self.report["calls"].append({
            "name": name, "status": status, "duration_ms": round((time.monotonic() - before) * 1000, 2),
            "response_bytes": len(raw), "response_sha256": hashlib.sha256(raw).hexdigest(),
            "content_encoding": response_headers.get("Content-Encoding", "identity"),
            "response_model": response.get("model") if isinstance(response.get("model"), str) else None,
            "usage": {key: value for key, value in usage.items() if numeric(value)},
            "answers": {key: {field: answer.get(field) for field in ("type", "choice", "noul", "score", "confidence", "probabilities", "legend") if field in answer}
                        for key, answer in response.get("answers", {}).items() if isinstance(answer, dict)}
                       if isinstance(response.get("answers"), dict) else {},
        })
        return status, response

    def run(self):
        binary = self.args.binary.resolve(strict=True)
        self.report["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        key = read_key(self.args.key_name, self.args.env_file)
        self.secrets.append(key)
        environment = {name: value for name, value in os.environ.items()
                       if name.lower() not in {"http_proxy", "https_proxy", "all_proxy", "typesafe_api_key"}
                       and name != self.args.key_name and value != key}
        environment["NO_PROXY"] = "127.0.0.1,localhost"
        environment["JEV_OBSERVER_DB_KEY"] = secrets.token_hex(32)
        self.secrets.append(environment["JEV_OBSERVER_DB_KEY"])
        version = subprocess.run([str(binary), "--version"], env=environment, capture_output=True, timeout=15)
        self.check(version.returncode == 0, "binary_version_command", required=True)
        self.report["binary_version"] = version.stdout.decode(errors="replace").strip()
        with tempfile.TemporaryDirectory(prefix="jev-live-openrouter-") as temporary:
            directory = Path(temporary) / ".jev-observer"
            directory.mkdir(mode=0o700)
            try:
                self.phase = "initial_startup"
                self.start(binary, directory, environment)
                status, html, _ = self.request("/")
                self.check(status == 200 and b"<html" in html.lower() and b"<script" in html.lower(), "embedded_dashboard_http")
                self.phase = "live_inference"
                status, _ = self.inference("direct_provider_key", REQUEST, key, direct=True)
                self.check(status == 200, "direct_provider_key_success")
                registration = self.api("/api/credentials", "PUT", {"api_key": key, "persist": False})
                local_token = registration.get("client_token", "")
                self.check(registration.get("storage") == "session" and local_token.startswith("jo_local_"), "session_registration", required=True)
                self.secrets.append(local_token)
                for number in (1, 2):
                    status, _ = self.inference(f"registered_repeat_{number}", REQUEST, local_token)
                    self.check(status == 200, f"registered_repeat_{number}_success")
                changed = copy.deepcopy(REQUEST)
                changed["questions"]["department"]["instructions"] += " Treat duplicate charges as billing issues."
                status, _ = self.inference("changed_choice_definition", changed, local_token)
                self.check(status == 200, "changed_definition_success")
                invalid = copy.deepcopy(REQUEST)
                invalid["model"] = "jev-observer-intentionally-invalid-model"
                status, _ = self.inference("invalid_provider_model", invalid, local_token)
                self.check(400 <= status < 500, "invalid_provider_model_is_4xx")
                before = self.api("/api/health")
                status, _ = self.inference("invalid_local_token", REQUEST, "jo_local_" + secrets.token_hex(32))
                after = self.api("/api/health")
                self.check(status in (401, 403) and after["forwarded"] == before["forwarded"], "invalid_local_token_rejected_before_upstream")
                self.phase = "stored_history"
                for _ in range(200):
                    health = self.api("/api/health")
                    if health["persisted"] >= 5:
                        break
                    time.sleep(0.05)
                self.report["health"] = health
                self.check(health["forwarded"] == 5 and health["persisted"] == 5, "five_upstream_calls_persisted")
                self.check(all(health.get(name) == 0 for name in ("dropped", "truncated", "write_failures")), "no_capture_gaps")
                dashboard = self.api("/api/dashboard?window=all")
                summary = dashboard["summary"]
                self.report["summary"] = summary
                self.report["group_count"] = len(dashboard["groups"])
                self.check(summary["request_count"] == 5 and summary["answer_count"] == 15 and summary["error_count"] == 1, "request_answer_error_counts")
                self.check(summary["incomplete_count"] == 0, "all_captures_complete")
                self.check(len(dashboard["groups"]) == 4, "four_definition_groups")
                self.check(sorted(group["valid_count"] for group in dashboard["groups"]) == [1, 3, 4, 4], "definition_group_valid_counts")
                records = [self.api("/api/requests/" + row["id"]) for row in dashboard["requests"]]
                successes = [record for record in records if record["status"] == 200]
                self.check(len(successes) == 4 and all(len(record["answers"]) == 3 and all(answer["valid"] for answer in record["answers"]) for record in successes), "all_three_primitives_valid")
                self.check(all(record["capture_complete"] for record in records), "record_capture_flags_complete")
                self.check(all(record.get("provider") == "openrouter" for record in records), "openrouter_provider_attribution")
                self.report["answer_warnings"] = [
                    {"key": answer["key"], "warnings": answer.get("warnings", [])}
                    for record in successes for answer in record["answers"] if answer.get("warnings")]
                self.check(all(record.get("state") is None and record.get("state_retained") is False for record in records), "input_state_not_retained")
                self.report["normalization_errors"] = [
                    {"status": record["status"], "normalization_error": record.get("normalization_error"),
                     "answers": [{"key": answer["key"], "valid": answer["valid"], "error": answer.get("error")} for answer in record["answers"]]}
                    for record in records if record["status"] == 200 and (record.get("normalization_error") or any(not answer["valid"] for answer in record["answers"]))]
                for field in ("input_tokens", "output_tokens"):
                    reported = [call["usage"].get(field) for call in self.report["calls"][:4]]
                    expected = sum(value for value in reported if numeric(value))
                    self.check(all(isinstance(value, int) and not isinstance(value, bool) and value >= 0 for value in reported), "provider_reports_" + field)
                    self.check(summary[field] == expected and sum(record[field] or 0 for record in records) == expected, "exact_" + field + "_accounting")
                self.report["cost_observation"] = {
                    "provider_usage_cost": [call["usage"].get("cost") for call in self.report["calls"][:4]],
                    "observer_cost_usd": [record.get("cost_usd") for record in successes],
                    "observer_usage_extra_cost": [record.get("usage_extra", {}).get("cost") for record in successes],
                    "note": "No configured price rates; records are not ordered to match calls. OpenRouter-reported USD cost is checked for exact accounting, not invoice reconciliation.",
                }
                reported_costs = [call["usage"].get("cost") for call in self.report["calls"][:4]]
                self.check(all(numeric(value) and value >= 0 for value in reported_costs), "provider_reports_valid_usd_cost")
                expected_cost = sum(value for value in reported_costs if numeric(value))
                self.check(numeric(summary.get("cost_usd")) and math.isclose(summary["cost_usd"], expected_cost, rel_tol=1e-9, abs_tol=1e-12), "exact_reported_cost_accounting")
                self.check(all(record.get("cost_basis") == "provider_reported" and record.get("cost_usd") == record.get("usage_extra", {}).get("cost") for record in successes), "reported_cost_basis_preserved")
                self.check(bool(successes), "successful_record_available_for_label", required=True)
                labeled_id = successes[0]["id"]
                self.api("/api/requests/" + labeled_id + "/label", "POST", {"key": "department", "label": "correct"})
                labeled = self.api("/api/requests/" + labeled_id)
                self.check(any(label["key"] == "department" and label["label"] == "correct" for label in labeled["labels"]), "review_label_saved")
                exports = {}
                for format_name in ("jsonl", "csv"):
                    status, raw, _ = self.request("/api/export?window=all&format=" + format_name)
                    self.check(status == 200, format_name + "_export_success")
                    exports[format_name] = raw.decode()
                    self.check(all(secret not in exports[format_name] for secret in self.secrets) and STATE_MARKER not in exports[format_name], format_name + "_excludes_credentials_and_input")
                self.check(len(list(csv.DictReader(io.StringIO(exports["csv"])))) == 5, "csv_contains_five_requests")
                exported = [json.loads(line) for line in exports["jsonl"].splitlines() if line.strip()]
                self.check(len(exported) == 5, "jsonl_contains_five_requests")
                self.report["reimport"] = self.api("/api/import", "POST", {"format": "observer-jsonl", "text": exports["jsonl"]})
                self.check(self.report["reimport"] == {"imported": 0, "duplicates": 5}, "reimport_is_deduplicated")
                prior_ids = sorted(record["id"] for record in records)
                self.phase = "before_restart"
                self.stop()
                self.check(not (directory / "history.sqlite").read_bytes().startswith(b"SQLite format 3"), "live_database_is_encrypted")
                self.phase = "same_key_restart"
                self.start(binary, directory, environment)
                reopened = self.api("/api/dashboard?window=all")
                self.check(reopened["summary"] == summary and sorted(row["id"] for row in reopened["requests"]) == prior_ids, "same_key_restart_retains_history")
                labeled = self.api("/api/requests/" + labeled_id)
                self.check(any(label["key"] == "department" and label["label"] == "correct" for label in labeled["labels"]), "same_key_restart_retains_labels")
                self.report["restart_summary"] = reopened["summary"]
            finally:
                self.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--env-file", type=Path, help="Read a literal key assignment; never execute the file")
    parser.add_argument("--key-name", default="OPENROUTER_API_KEY")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    smoke = Smoke(args)
    try:
        smoke.run()
    except Exception as error:
        detail = {"phase": smoke.phase, "type": type(error).__name__}
        if isinstance(error, CheckFailed):
            detail["check"] = str(error)
        smoke.report["errors"].append(detail)
    finally:
        smoke.report["passed"] = not smoke.report["errors"] and all(smoke.report["checks"].values())
        smoke.report["finished_at"] = datetime.now(timezone.utc).isoformat()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(smoke.scrub(smoke.report), indent=2, allow_nan=False) + "\n")
    print(json.dumps({"passed": smoke.report["passed"], "calls": len(smoke.report["calls"]),
                      "failed_checks": [name for name, passed in smoke.report["checks"].items() if not passed],
                      "error_types": [error["type"] for error in smoke.report["errors"]]}))
    return 0 if smoke.report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
