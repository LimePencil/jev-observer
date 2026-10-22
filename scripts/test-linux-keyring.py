#!/usr/bin/env python3
"""Exercise old/new Secret Service credentials on an isolated session bus and keyring."""
import argparse
from datetime import datetime, timezone
import hashlib
from http.server import ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import threading
import time
import urllib.error

spec = importlib.util.spec_from_file_location("upgrade", Path(__file__).with_name("test-upgrade.py"))
upgrade = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upgrade)

CONTROL_SOCKET_LIMIT = 107  # Linux sockaddr_un.sun_path includes a trailing NUL.
DIAGNOSTIC_LOG_LIMIT = 16 * 1024


def control_socket_bytes(control):
    return len(os.fsencode(control.absolute() / "control"))


def log_tail(path):
    """Retain bounded synthetic service diagnostics before workspace cleanup."""
    try:
        with path.open("rb") as stream:
            size = os.fstat(stream.fileno()).st_size
            stream.seek(max(0, size - DIAGNOSTIC_LOG_LIMIT))
            data = stream.read(DIAGNOSTIC_LOG_LIMIT)
        return {"bytes": size, "truncated": size > DIAGNOSTIC_LOG_LIMIT,
                "tail": data.decode("utf-8", errors="replace")}
    except OSError as error:
        return {"error_type": type(error).__name__}


def retain_service_diagnostics(report, root, bus, daemon):
    diagnostics = report["service_diagnostics"] = {}
    for name, process in [("bus", bus), ("keyring", daemon)]:
        diagnostics[name + "_exit_code"] = cleanup(
            report, name + "_status", lambda: process.poll() if process is not None else None)
        value = log_tail(root / ("bus.log" if name == "bus" else "keyring.log"))
        diagnostics[name + "_log"] = value
        if "error_type" in value:
            report["passed"] = False
            report.setdefault("diagnostic_failures", []).append({"stage": name + "_log", **value})


def record_failure(report, phase, error):
    report["passed"] = False
    if "failure" not in report:
        report["failure"] = {"phase": phase, "type": type(error).__name__}
        if isinstance(error, AssertionError):
            report["failure"]["check"] = str(error)
            if str(error) == "private_control_socket_path_fits":
                report["failure"]["hint"] = "Use a shorter TMPDIR for the private keyring control socket."


def cleanup(report, stage, operation):
    try:
        return operation()
    except Exception as error:
        report["passed"] = False
        report.setdefault("cleanup_failures", []).append({"stage": stage, "type": type(error).__name__})
        return None


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise AssertionError("owned_daemon_required_kill")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--backend-tests", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.baseline = args.baseline.resolve(strict=True)
    args.candidate = args.candidate.resolve(strict=True)
    if args.backend_tests:
        args.backend_tests = args.backend_tests.resolve(strict=True)
    args.timeout = 10
    app = upgrade.Upgrade(args)
    report = {"checked_at": datetime.now(timezone.utc).isoformat(), "passed": False,
              "scope": "Synthetic GNOME Secret Service on a new private D-Bus session and XDG directories; no personal keyring or provider access.",
              "baseline_sha256": hashlib.sha256(args.baseline.read_bytes()).hexdigest(),
              "candidate_sha256": hashlib.sha256(args.candidate.read_bytes()).hexdigest(),
              "checks": app.report["checks"]}
    server = None
    server_started = False
    bus = daemon = None
    try:
        with tempfile.TemporaryDirectory(prefix="observer-keyring-") as temporary:
            root = Path(temporary)
            app.directory = root / "workspace"
            app.directory.mkdir(mode=0o700)
            app.database = app.directory / "history.sqlite"
            for key, name in [("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                              ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")]:
                path = root / name
                path.mkdir(mode=0o700)
                app.environment[key] = str(path)
            for key in list(app.environment):
                if key.startswith("DBUS_") or key.startswith("GNOME_KEYRING_") or key in {"DISPLAY", "WAYLAND_DISPLAY"}:
                    app.environment.pop(key)
            control = root / "control"
            control.mkdir(mode=0o700)
            report["control_socket_path_bytes"] = control_socket_bytes(control)
            report["control_socket_path_limit"] = CONTROL_SOCKET_LIMIT
            # GNOME Keyring truncates an oversized bind path, but unlinks the
            # original path on shutdown. Reject it before creating any service
            # so a restart cannot encounter an orphaned sibling socket.
            app.check(report["control_socket_path_bytes"] <= CONTROL_SOCKET_LIMIT,
                      "private_control_socket_path_fits")
            app.environment["GNOME_KEYRING_CONTROL"] = str(control)
            server = ThreadingHTTPServer(("127.0.0.1", 0), upgrade.Mock)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            server_started = True
            app.upstream = f"http://127.0.0.1:{server.server_port}/v1/systemone"
            with (root / "bus.log").open("wb") as bus_log, (root / "keyring.log").open("ab") as daemon_log:
                try:
                    bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1"],
                                           env=app.environment, stdout=subprocess.PIPE, stderr=bus_log)
                    app.check(bool(select.select([bus.stdout], [], [], 10)[0]), "private_bus_ready")
                    address = bus.stdout.readline().decode().strip()
                    app.check(address.startswith("unix:"), "private_bus_address")
                    app.environment["DBUS_SESSION_BUS_ADDRESS"] = address

                    def start_daemon():
                        nonlocal daemon
                        daemon = subprocess.Popen(["gnome-keyring-daemon", "--foreground", "--unlock", "--components=secrets",
                                                   "--control-directory", str(control)], env=app.environment,
                                                  stdin=subprocess.PIPE, stdout=daemon_log, stderr=daemon_log)
                        daemon.stdin.write(b"synthetic-keyring-unlock-password")
                        daemon.stdin.close()
                        for _ in range(100):
                            app.check(daemon.poll() is None, "private_keyring_running")
                            state = subprocess.run(["dbus-send", "--session", "--print-reply", "--dest=org.freedesktop.DBus",
                                                    "/org/freedesktop/DBus", "org.freedesktop.DBus.NameHasOwner",
                                                    "string:org.freedesktop.secrets"], env=app.environment,
                                                   capture_output=True, timeout=2)
                            if state.returncode == 0 and b"boolean true" in state.stdout:
                                return
                            time.sleep(.1)
                        raise AssertionError("private_keyring_startup_deadline")

                    def configured(expected):
                        value = app.api("/api/credentials")
                        app.check(value == {"configured": expected, "storage": "system" if expected else "none"},
                                  app.phase + "_credential_status")

                    def forward(token, accepted=True):
                        try:
                            app.api("/v1/systemone", "POST", upgrade.REQUEST,
                                    {"Authorization": "Bearer " + token, "X-Observer-Access": app.token})
                        except urllib.error.HTTPError as error:
                            app.check(not accepted and error.code == 401, app.phase + "_obsolete_token_rejected")
                        else:
                            app.check(accepted, app.phase + "_saved_key_forwards_to_loopback")

                    start_daemon()
                    if args.backend_tests:
                        with (root / "backend-tests.log").open("wb") as log:
                            result = subprocess.run([str(args.backend_tests), "--test-threads=1"],
                                                    env=app.environment, stdout=log, stderr=subprocess.STDOUT, timeout=120)
                        app.check(result.returncode == 0, "upstream_backend_regressions")
                    app.phase = "old_write"
                    app.start(args.baseline)
                    configured(False)
                    first = app.api("/api/credentials", "PUT", {"api_key": upgrade.KEY, "persist": True})
                    app.check(first["storage"] == "system", "old_saved_to_system")
                    forward(first["client_token"])
                    app.stop()
                    stop(daemon)
                    start_daemon()
                    app.phase = "new_read_after_keyring_restart"
                    app.start(args.candidate)
                    configured(True)
                    forward(first["client_token"])
                    second = app.api("/api/credentials", "PUT", {"api_key": upgrade.KEY, "persist": True})
                    app.check(second["client_token"] != first["client_token"], "new_rotation_changes_token")
                    forward(first["client_token"], False)
                    forward(second["client_token"])
                    app.stop()
                    stop(daemon)
                    start_daemon()
                    app.phase = "old_read_new_write_after_keyring_restart"
                    app.start(args.baseline)
                    configured(True)
                    forward(second["client_token"])
                    app.stop()
                    app.phase = "new_delete"
                    app.start(args.candidate)
                    app.api("/api/credentials", "DELETE")
                    configured(False)
                    forward(second["client_token"], False)
                    app.stop()
                    app.phase = "old_read_after_delete"
                    app.start(args.baseline)
                    configured(False)
                    app.stop()
                    app.phase = "new_session_revocation"
                    app.start(args.candidate)
                    app.api("/api/credentials", "PUT", {"api_key": upgrade.KEY, "persist": True})
                    session = app.api("/api/credentials", "PUT", {"api_key": upgrade.KEY, "persist": False})
                    app.check(session["storage"] == "session", "session_replaces_saved_entry")
                    app.stop()
                    app.phase = "old_read_after_session_revocation"
                    app.start(args.baseline)
                    configured(False)
                    app.stop()
                    report["loopback_calls"] = upgrade.Mock.calls
                    app.check(upgrade.Mock.calls == 4, "only_four_loopback_provider_calls")
                    report["passed"] = True
                except BaseException as error:
                    record_failure(report, app.phase, error)
                finally:
                    cleanup(report, "observer_stop", app.stop)
                    cleanup(report, "keyring_stop", lambda: stop(daemon))
                    cleanup(report, "bus_stop", lambda: stop(bus))
                    if bus is not None and bus.stdout:
                        cleanup(report, "bus_stdout_close", bus.stdout.close)
                    cleanup(report, "bus_log_flush", bus_log.flush)
                    cleanup(report, "keyring_log_flush", daemon_log.flush)
                    retain_service_diagnostics(report, root, bus, daemon)
    except BaseException as error:
        if "failure" in report:
            report["passed"] = False
            report.setdefault("cleanup_failures", []).append({"stage": "workspace_exit", "type": type(error).__name__})
        else:
            record_failure(report, app.phase, error)
    finally:
        if server is not None:
            if server_started:
                cleanup(report, "mock_server_shutdown", server.shutdown)
            cleanup(report, "mock_server_close", server.server_close)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "checks": len(report["checks"]), "failure": report.get("failure")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
