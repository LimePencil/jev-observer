#!/usr/bin/env python3
"""Compare two local Observer binaries against isolated backups of a quiet fixture.

Only loopback GET /api/health and GET /api/dashboard are issued. The fixture is
never opened by SQLite: a byte copy (including any WAL) is staged first, and
SQLite's backup API produces the two independent benchmark databases from it.

Use --filters=all,search --samples-per-filter=5 for repeated comparisons of only
those filters. An omitted all-history filter still receives one count check per
binary, reported separately from the selected timing samples.
"""

import argparse
import base64
import hashlib
import http.client
import json
import math
import os
from pathlib import Path
import platform
import shutil
import socket
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from contextlib import closing
from datetime import datetime, timezone
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[1]
FILTERS = [
    ("all", {"window": "all"}),
    ("24h", {"window": "24h"}),
    ("1h", {"window": "1h"}),
    ("source", {"window": "all", "source": "benchmark"}),
    ("model", {"window": "all", "model": "jev-benchmark-1"}),
    ("search", {"window": "all", "search": "urgency"}),
    ("error", {"window": "all", "status": "error"}),
]


def filter_selection(value):
    names = [name.strip() for name in value.split(",")]
    available = dict(FILTERS)
    if not names or any(name not in available for name in names):
        raise argparse.ArgumentTypeError("choose comma-separated filters from: " + ",".join(available))
    if len(set(names)) != len(names):
        raise argparse.ArgumentTypeError("filter names must not repeat")
    return [(name, available[name]) for name in names]


def sample_count(label, options):
    if options.samples_per_filter is not None:
        return options.samples_per_filter
    return options.samples if label == "all" else 1


def now():
    return datetime.now(timezone.utc).isoformat()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def manifest(path):
    result = {}
    for suffix in ("", "-wal", "-shm"):
        file = Path(str(path) + suffix)
        if not file.exists():
            result[suffix or "database"] = None
            continue
        before = file.stat()
        digest = sha256(file)
        after = file.stat()
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise RuntimeError(f"Fixture changed while hashing {file}; stop its writer first")
        result[suffix or "database"] = {
            "bytes": after.st_size, "mtime_ns": after.st_mtime_ns, "sha256": digest,
        }
    return result


def canonical(payload):
    # Array order, numbers, and every other field remain significant.
    value = {key: item for key, item in payload.items() if key not in ("generated_at", "health")}
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
    return value, hashlib.sha256(encoded.encode()).hexdigest()


def differences(left, right, path="$", found=None):
    found = [] if found is None else found
    if len(found) >= 10:
        return found
    if type(left) is not type(right):
        found.append({"path": path, "baseline": left, "candidate": right})
    elif isinstance(left, dict):
        for key in sorted(left.keys() | right.keys()):
            if key not in left or key not in right:
                found.append({"path": f"{path}.{key}", "baseline_present": key in left, "candidate_present": key in right})
            else:
                differences(left[key], right[key], f"{path}.{key}", found)
            if len(found) >= 10:
                break
    elif isinstance(left, list):
        if len(left) != len(right):
            found.append({"path": path, "baseline_length": len(left), "candidate_length": len(right)})
        for index, (first, second) in enumerate(zip(left, right)):
            differences(first, second, f"{path}[{index}]", found)
            if len(found) >= 10:
                break
    elif left != right:
        found.append({"path": path, "baseline": left, "candidate": right})
    return found


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


authorization = ""


def exchange(connection, path):
    started = time.perf_counter_ns()
    connection.request("GET", path, headers={"Authorization": authorization})
    response = connection.getresponse()
    body = response.read()
    elapsed = (time.perf_counter_ns() - started) / 1_000_000
    if response.status != 200:
        raise RuntimeError(f"GET {path} returned HTTP {response.status}: {body[:512]!r}")
    return json.loads(body), elapsed


def prepare_copies(fixture, directory):
    # Staging avoids even SQLite shared-memory lock writes on the original.
    staged = directory / "staged.sqlite"
    shutil.copyfile(fixture, staged)
    wal = Path(str(fixture) + "-wal")
    if wal.exists():
        shutil.copyfile(wal, Path(str(staged) + "-wal"))
    copies = {}
    with closing(sqlite3.connect(staged.as_uri() + "?mode=ro", uri=True)) as source:
        row = source.execute("SELECT count(*),coalesce(sum(event_kind='request'),0),coalesce(sum(event_kind='application_action'),0),min(timestamp),max(timestamp) FROM requests").fetchone()
        counts = dict(zip(("records", "requests", "actions", "oldest_timestamp", "latest_timestamp"), row))
        for table in ("answers", "groups", "labels"):
            counts[table] = source.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
        for name in ("baseline", "candidate"):
            destination = directory / f"{name}.sqlite"
            with closing(sqlite3.connect(destination)) as target:
                source.backup(target)
            copies[name] = destination
    return copies, counts


def run_binary(name, binary, database, counts, options, directory):
    global authorization
    authorization = ""
    port = free_port()
    oldest = counts["oldest_timestamp"]
    retention = max(7, math.ceil((time.time() * 1000 - oldest) / 86_400_000) + 2) if oldest is not None else 7
    command = [str(binary), "--port", str(port), "--db", str(database),
               "--upstream", "http://127.0.0.1:9/v1/systemone", "--retention-days", str(retention),
               "--max-records", str(max(1, counts["records"] + 1))]
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ("http_proxy", "https_proxy", "all_proxy", "typesafe_api_key")}
    environment["NO_PROXY"] = "127.0.0.1,localhost"
    environment["JEV_OBSERVER_DB_KEY"] = os.urandom(32).hex()
    report = {"binary": str(binary), "binary_sha256": sha256(binary),
              "database_sha256_before_start": sha256(database), "command": command,
              "started_at": now(), "filters": {}}
    payloads = {}
    log_path = directory / f"{name}.log"
    token_path = database.with_suffix(".access-token")
    with log_path.open("wb") as log:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, cwd=ROOT, env=environment)
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=options.timeout)
        try:
            deadline = time.monotonic() + options.timeout
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"{name} exited at startup: {log_path.read_text(errors='replace')[-4000:]}")
                try:
                    if token_path.exists():
                        token = token_path.read_text()
                        authorization = "Basic " + base64.b64encode(f"observer:{token}".encode()).decode()
                    health, _ = exchange(connection, "/api/health")
                    break
                except (OSError, http.client.HTTPException):
                    connection.close()
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f"{name} startup timed out: {log_path.read_text(errors='replace')[-4000:]}")
                    time.sleep(0.1)
            if not any(label == "all" for label, _ in options.filters):
                response, elapsed = exchange(connection, "/api/dashboard?window=all")
                payloads["all"], digest = canonical(response)
                report["fixture_count_check"] = {
                    "parameters": {"window": "all"}, "elapsed_ms": round(elapsed, 3),
                    "response_sha256": digest, "included_in_selected_samples": False,
                    "counts": {key: payloads["all"]["summary"].get(key)
                               for key in ("request_count", "action_count", "answer_count")},
                }
            for label, parameters in options.filters:
                path = "/api/dashboard?" + urlencode(parameters)
                result = {"parameters": parameters, "samples_ms": [], "response_sha256": []}
                if label == "all":
                    warm, elapsed = exchange(connection, path)
                    _, digest = canonical(warm)
                    result.update(warmup_ms=elapsed, warmup_sha256=digest)
                for _ in range(sample_count(label, options)):
                    response, elapsed = exchange(connection, path)
                    payload, digest = canonical(response)
                    result["samples_ms"].append(round(elapsed, 3))
                    result["response_sha256"].append(digest)
                    payloads.setdefault(label, payload)
                result["median_ms"] = round(statistics.median(result["samples_ms"]), 3)
                result["stable_responses"] = len(set(result["response_sha256"])) == 1
                if label == "all":
                    result["stable_responses"] &= result["warmup_sha256"] == result["response_sha256"][0]
                result["counts"] = {key: payload["summary"].get(key) for key in ("request_count", "action_count", "answer_count", "error_count")}
                result["counts"].update(groups=len(payload["groups"]), request_feed=len(payload["requests"]))
                report["filters"][label] = result
                print(f"{name} {label}: {result['median_ms']:.3f} ms median, sha256={result['response_sha256'][0]}", flush=True)
            health, _ = exchange(connection, "/api/health")
            report["no_forwarded_calls"] = health.get("forwarded") == 0
            report["fixture_counts_preserved"] = (
                payloads["all"]["summary"]["request_count"] == counts["requests"]
                and payloads["all"]["summary"].get("action_count", 0) == counts["actions"]
                and payloads["all"]["summary"]["answer_count"] == counts["answers"]
            )
        finally:
            connection.close()
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
    report["finished_at"] = now()
    report["binary_unchanged"] = sha256(binary) == report["binary_sha256"]
    return report, payloads


def environment_info():
    cpu = platform.processor()
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        cpu = next((line.split(":", 1)[1].strip() for line in cpuinfo.read_text().splitlines() if line.startswith("model name")), cpu)
    result = {"python": sys.version, "sqlite": sqlite3.sqlite_version, "platform": platform.platform(),
              "cpu": cpu, "logical_cpus": os.cpu_count()}
    if hasattr(os, "getloadavg"):
        result["load_average_at_start"] = list(os.getloadavg())
    if hasattr(os, "sysconf"):
        result["total_memory_bytes"] = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, default=ROOT / ".jev-observer/baseline-observer")
    parser.add_argument("--candidate", type=Path, default=ROOT / "target/release/jev-observer")
    parser.add_argument("--fixture", type=Path, required=True, help="Quiescent SQLite fixture; never opened or modified in place")
    parser.add_argument("--output", type=Path, default=ROOT / "reports/benchmarks/dashboard-comparison.json")
    parser.add_argument("--samples", type=int, default=5, help="Timed all-history samples after one warm-up (default: 5)")
    parser.add_argument("--filters", type=filter_selection, default=",".join(label for label, _ in FILTERS),
                        help="Comma-separated filters in execution order: all,24h,1h,source,model,search,error (default: all seven)")
    parser.add_argument("--samples-per-filter", type=int,
                        help="Timed samples for every selected filter, 1–100; overrides --samples (default: five for all, one for others)")
    parser.add_argument("--timeout", type=float, default=120, help="Startup and individual HTTP timeout in seconds")
    options = parser.parse_args()
    if options.samples < 1 or not math.isfinite(options.timeout) or options.timeout <= 0:
        parser.error("--samples and --timeout must be positive")
    if options.samples_per_filter is not None and not 1 <= options.samples_per_filter <= 100:
        parser.error("--samples-per-filter must be between 1 and 100")
    for key in ("baseline", "candidate", "fixture", "output"):
        setattr(options, key, getattr(options, key).resolve())
    protected = {options.baseline, options.candidate, *(Path(str(options.fixture) + suffix) for suffix in ("", "-wal", "-shm"))}
    if options.output in protected:
        parser.error("Output must not overwrite a binary or the source fixture")
    for file in (options.baseline, options.candidate, options.fixture):
        if not file.is_file():
            parser.error(f"Missing file: {file}")
    report = {"started_at": now(), "environment": environment_info(), "fixture": str(options.fixture),
              "selected_filters": [label for label, _ in options.filters],
              "samples_per_filter": {label: sample_count(label, options) for label, _ in options.filters},
              "excluded_top_level_fields": ["generated_at", "health"], "complete": False,
              "notes": ["Binaries run sequentially on independent SQLite backup copies; no provider requests are sent.",
                        "Only object-key order is normalized. Array order and all remaining JSON values compare exactly; no floating-point tolerance.",
                        "Timings include loopback HTTP response transfer and exclude client JSON parsing and hashing. All-history gets one warm-up and five samples by default; other filters get one query each.",
                        "Selected filters run in the supplied order. When all-history is omitted, one separate all-history count check runs first and can warm the database cache; it is excluded from selected timing samples and comparisons.",
                        "Each server uses its current clock. Rows crossing 1h/24h boundaries during the run can cause a real comparison failure; no timestamp fields are silently removed.",
                        "The source database and WAL are byte-staged before SQLite backup, preventing writes even to original shared-memory sidecars."]}
    before = None
    try:
        before = manifest(options.fixture)
        report["source_before"] = before
        work_root = ROOT / ".jev-observer"
        work_root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="dashboard-comparison-", dir=work_root) as temporary:
            directory = Path(temporary)
            copies, counts = prepare_copies(options.fixture, directory)
            report["fixture_counts"] = counts
            if manifest(options.fixture) != before:
                raise RuntimeError("Source fixture changed while taking backups; rerun after stopping all writers")
            baseline, first = run_binary("baseline", options.baseline, copies["baseline"], counts, options, directory)
            report["baseline"] = baseline
            candidate, second = run_binary("candidate", options.candidate, copies["candidate"], counts, options, directory)
            report["candidate"] = candidate
            report["comparisons"] = {}
            for label, _ in options.filters:
                left, right = baseline["filters"][label], candidate["filters"][label]
                equal = left["response_sha256"][0] == right["response_sha256"][0]
                report["comparisons"][label] = {"equal": equal, "stable_responses": left["stable_responses"] and right["stable_responses"],
                    "baseline_median_ms": left["median_ms"], "candidate_median_ms": right["median_ms"],
                    "speedup": round(left["median_ms"] / right["median_ms"], 3) if right["median_ms"] else None,
                    "first_differences": [] if equal else differences(first[label], second[label])}
            report["complete"] = all(item["equal"] and item["stable_responses"] for item in report["comparisons"].values()) and all(
                run["no_forwarded_calls"] and run["fixture_counts_preserved"] and run["binary_unchanged"] for run in (baseline, candidate))
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        if before is not None:
            try:
                report["source_after"] = manifest(options.fixture)
                report["source_unchanged"] = report["source_after"] == before
            except Exception as error:
                report["source_unchanged"] = False
                report["source_check_error"] = str(error)
            report["complete"] &= report["source_unchanged"]
        report["finished_at"] = now()
        options.output.parent.mkdir(parents=True, exist_ok=True)
        options.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(f"Saved {options.output}; complete={report['complete']}")
    if "error" in report:
        print(report["error"], file=sys.stderr)
    return 0 if report["complete"] else 1


if __name__ == "__main__":
    sys.exit(main())
