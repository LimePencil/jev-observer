#!/usr/bin/env python3
"""Measure encrypted synthetic history queries and verify retention and paging.

One loopback request seeds a SQLCipher fixture. The SQLCipher helper expands its
stored rows; this is a quiet database query check, not an ingestion benchmark.
"""
import argparse
from datetime import datetime, timezone
import hashlib
from http.server import ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import threading
import time

spec = importlib.util.spec_from_file_location("upgrade", Path(__file__).with_name("test-upgrade.py"))
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--records", type=int, default=100_000)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--timeout", type=int, default=60, help="Per-query timeout in seconds")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 2000 <= args.records <= 200_000 or not 1 <= args.samples <= 20:
        parser.error("Use 2000–200000 records and 1–20 samples")
    if not 1 <= args.timeout <= 300:
        parser.error("Use a query timeout between 1 and 300 seconds")
    args.binary = args.binary.resolve(strict=True)
    args.probe = args.probe.resolve(strict=True)
    app = upgrade.Upgrade(args)
    report = {"checked_at": datetime.now(timezone.utc).isoformat(), "scope": __doc__.strip(),
              "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "probe_sha256": hashlib.sha256(args.probe.read_bytes()).hexdigest(),
              "platform": platform.platform(), "records": args.records, "answers": args.records * 3,
              "definition_groups": 3, "storage": "SQLCipher", "queries": {}, "passed": False,
              "notes": ["Shared host; results do not establish a capacity limit or compare provider latency.",
                        "Fixture contains three repeated definitions and small answers; higher cardinalities and larger payloads need separate measurements."]}
    server = ThreadingHTTPServer(("127.0.0.1", 0), upgrade.Mock)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    app.upstream = f"http://127.0.0.1:{server.server_port}/v1/systemone"
    try:
        with tempfile.TemporaryDirectory(prefix="observer-history-") as temporary:
            app.directory = Path(temporary) / ".jev-observer"
            app.directory.mkdir(mode=0o700)
            app.database = app.directory / "history.sqlite"
            try:
                app.phase = "seed"
                app.start(args.binary)
                app.api("/v1/systemone", "POST", upgrade.REQUEST,
                        {"Authorization": "Bearer " + upgrade.KEY, "X-Observer-Access": app.token, "X-Observer-Source": "upgrade-fixture"})
                for _ in range(200):
                    if app.api("/api/health")["persisted"] == 1:
                        break
                    time.sleep(0.05)
                app.check(app.api("/api/health")["persisted"] == 1, "template_capture_persisted")
                app.stop()
                seeded = subprocess.run([str(args.probe), str(app.database), "seed-history", str(args.records)],
                                        env=app.environment, capture_output=True, timeout=180)
                app.check(seeded.returncode == 0, "encrypted_fixture_expanded")
                with app.database.open("rb") as source:
                    app.check(source.read(16) != b"SQLite format 3\0", "fixture_has_no_plaintext_header")
                report["database_bytes"] = app.database.stat().st_size
                app.phase = "large_history"
                app.start(args.binary)
                routes = {
                    "all": "/api/dashboard?window=all",
                    "search": "/api/dashboard?window=all&search=urgency",
                    "source": "/api/dashboard?window=all&source=upgrade-fixture",
                    "failures": "/api/dashboard?window=all&status=error",
                }
                snapshots = {}
                for name, route in routes.items():
                    app.api(route)  # Explicit warm-up excluded from samples.
                    samples = []
                    for _ in range(args.samples):
                        started = time.perf_counter()
                        snapshots[name] = app.api(route)
                        samples.append(round((time.perf_counter() - started) * 1000, 3))
                    expected = 0 if name == "failures" else args.records
                    app.check(snapshots[name]["summary"]["request_count"] == expected, name + "_count_exact")
                    report["queries"][name] = {"samples_ms": samples, "median_ms": statistics.median(samples), "requests": expected}
                first = snapshots["all"]
                app.check(first["summary"]["answer_count"] == args.records * 3 and len(first["groups"]) == 3, "answers_and_groups_exact")
                cursor = first["feed_next_cursor"]
                app.check(bool(cursor) and len(first["requests"]) == 100, "first_page_bounded")
                second = app.api("/api/dashboard?window=all&request_cursor=" + cursor)
                app.check(len(second["requests"]) == 100 and not ({row["id"] for row in first["requests"]} & {row["id"] for row in second["requests"]}), "request_pages_do_not_repeat")
                newest = first["requests"][0]["timestamp"]
                interval = app.api(f"/api/dashboard?window=all&from={newest - 49_000}&to={newest}")
                app.check(interval["summary"]["request_count"] == 50, "inclusive_date_range_exact")
                selected = app.api("/api/dashboard?window=all&group_search=urgency")
                app.check(len(selected["groups"]) == 1 and selected["groups"][0]["key"] == "urgency", "group_search_matches_definition")
                app.check(sum(bucket["requests"] for bucket in first["timeline"]) == args.records, "timeline_covers_entire_history")
                app.check(app.api("/api/health")["forwarded"] == 0, "query_phase_has_no_inference")
                app.stop()
                app.probe("age-history")
                app.phase = "retention"
                app.start(args.binary)
                retained = app.api("/api/dashboard?window=all")["summary"]["request_count"]
                app.check(retained == args.records - args.records // 10, "expired_tenth_removed_on_startup")
                app.stop()
                app.phase = "record_cap"
                app.start(args.binary, ["--max-records", "1000"])
                capped = app.api("/api/dashboard?window=all")["summary"]["request_count"]
                app.check(capped == 1000, "record_cap_enforced_on_startup")
                app.check(upgrade.Mock.calls == 1, "only_one_loopback_fixture_call")
                report["retention"] = {"before": args.records, "after_expiration": retained, "after_record_cap": capped}
            finally:
                app.stop()
        report["passed"] = True
    except Exception as error:
        report["failure"] = {"phase": app.phase, "type": type(error).__name__}
        if isinstance(error, AssertionError):
            report["failure"]["check"] = str(error)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    report["checks"] = app.report["checks"]
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "queries": report["queries"], "failure": report.get("failure")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
