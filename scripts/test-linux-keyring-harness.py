#!/usr/bin/env python3
"""Test keyring fixture failures without starting a bus, keyring or Observer."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("keyring_test", Path(__file__).with_name("test-linux-keyring.py"))
keyring = importlib.util.module_from_spec(spec)
spec.loader.exec_module(keyring)
CANARY = "private-exception-detail-must-not-enter-report"


class HarnessFailures(unittest.TestCase):
    def exercise(self, *, long_path=False, path_bytes=None, cleanup_failure=False,
                 read_failure=False, workspace_failure=False, thread_failure=False):
        with tempfile.TemporaryDirectory(prefix="kh-") as parent:
            parent = Path(parent)
            root = parent / ("p" * 120 if long_path else "w")
            output = parent / "report.json"
            server = mock.Mock(server_port=1)
            thread = mock.Mock()
            if thread_failure:
                thread.start.side_effect = RuntimeError(CANARY)

            @contextlib.contextmanager
            def workspace(*args, **kwargs):
                root.mkdir(mode=0o700)
                try:
                    yield str(root)
                finally:
                    shutil.rmtree(root)
                    if workspace_failure:
                        raise OSError(CANARY)

            children = []

            def popen(argv, **kwargs):
                name = argv[0]
                stream = kwargs["stderr"] if name == "dbus-daemon" else kwargs["stdout"]
                stream.write(b"ignored-prefix:" + b"x" * (20 * 1024) + (name + ": diagnostic-tail\n").encode())
                stream.flush()
                process = SimpleNamespace(name=name, poll=lambda: 0, returncode=0,
                                          stdout=io.BytesIO(b"unix:path=/synthetic-private-bus\n"),
                                          stdin=io.BytesIO())
                children.append(process)
                return process

            stops = []

            def stop(process):
                if process is not None:
                    stops.append(process.name)
                if cleanup_failure:
                    raise OSError(CANARY)

            original_open = Path.open

            def opened(path, mode="r", *args, **kwargs):
                if read_failure and path == root / "keyring.log" and mode == "rb":
                    raise PermissionError(CANARY)
                return original_open(path, mode, *args, **kwargs)

            argv = ["test-linux-keyring", "--baseline", __file__, "--candidate", __file__, "--output", str(output)]
            with contextlib.ExitStack() as patches:
                patches.enter_context(mock.patch("sys.argv", argv))
                patches.enter_context(mock.patch.object(keyring.tempfile, "TemporaryDirectory", workspace))
                create_server = patches.enter_context(mock.patch.object(keyring, "ThreadingHTTPServer", return_value=server))
                patches.enter_context(mock.patch.object(keyring.threading, "Thread", return_value=thread))
                start_process = patches.enter_context(mock.patch.object(keyring.subprocess, "Popen", side_effect=popen))
                patches.enter_context(mock.patch.object(keyring.select, "select", return_value=([object()], [], [])))
                patches.enter_context(mock.patch.object(keyring, "stop", side_effect=stop))
                patches.enter_context(mock.patch.object(Path, "open", opened))
                if cleanup_failure:
                    patches.enter_context(mock.patch.object(keyring.upgrade.Upgrade, "stop", side_effect=RuntimeError(CANARY)))
                if path_bytes is not None:
                    patches.enter_context(mock.patch.object(keyring, "control_socket_bytes", return_value=path_bytes))
                stdout = patches.enter_context(contextlib.redirect_stdout(io.StringIO()))
                self.assertEqual(keyring.main(), 1)
            report = json.loads(output.read_text())
            self.assertFalse(report["passed"])
            self.assertFalse(root.exists())
            self.assertNotIn(CANARY, output.read_text() + stdout.getvalue())
            return report, server, create_server, start_process, stops

    def test_native_byte_count_includes_control_suffix_and_unicode_bytes(self):
        self.assertEqual(keyring.control_socket_bytes(Path("/" + "a" * 98)), 107)
        self.assertEqual(keyring.control_socket_bytes(Path("/" + "a" * 99)), 108)
        self.assertEqual(keyring.control_socket_bytes(Path("/" + "é" * 49)), 107)
        self.assertEqual(keyring.control_socket_bytes(Path("/" + "é" * 50)), 109)

    def test_long_path_fails_before_any_service_is_constructed(self):
        report, _, server, processes, stops = self.exercise(long_path=True)
        self.assertGreater(report["control_socket_path_bytes"], 107)
        self.assertEqual(report["failure"]["check"], "private_control_socket_path_fits")
        self.assertIn("shorter TMPDIR", report["failure"]["hint"])
        server.assert_not_called()
        processes.assert_not_called()
        self.assertEqual(stops, [])

    def test_exact_limit_reaches_service_and_retains_bounded_logs_before_cleanup(self):
        report, server, _, processes, stops = self.exercise(path_bytes=107)
        self.assertTrue(report["checks"]["private_control_socket_path_fits"])
        self.assertEqual(report["failure"]["check"], "private_keyring_running")
        self.assertEqual(processes.call_count, 2)
        self.assertEqual(stops, ["gnome-keyring-daemon", "dbus-daemon"])
        for name in ("bus", "keyring"):
            log = report["service_diagnostics"][name + "_log"]
            self.assertTrue(log["truncated"])
            self.assertEqual(len(log["tail"].encode()), 16 * 1024)
            self.assertNotIn("ignored-prefix", log["tail"])
            self.assertIn("diagnostic-tail", log["tail"])
            self.assertEqual(report["service_diagnostics"][name + "_exit_code"], 0)
        server.shutdown.assert_called_once()
        server.server_close.assert_called_once()

    def test_one_byte_over_limit_does_not_start_services(self):
        report, _, server, processes, _ = self.exercise(path_bytes=108)
        self.assertEqual(report["failure"]["check"], "private_control_socket_path_fits")
        server.assert_not_called()
        processes.assert_not_called()

    def test_cleanup_failures_preserve_first_failure_and_attempt_every_stop(self):
        report, server, _, _, stops = self.exercise(path_bytes=107, cleanup_failure=True)
        self.assertEqual(report["failure"]["check"], "private_keyring_running")
        self.assertEqual(stops, ["gnome-keyring-daemon", "dbus-daemon"])
        self.assertEqual([item["stage"] for item in report["cleanup_failures"]],
                         ["observer_stop", "keyring_stop", "bus_stop"])
        self.assertIn("diagnostic-tail", report["service_diagnostics"]["keyring_log"]["tail"])
        server.server_close.assert_called_once()

    def test_failed_diagnostic_read_keeps_original_failure_and_other_log(self):
        report, _, _, _, _ = self.exercise(path_bytes=107, read_failure=True)
        self.assertEqual(report["failure"]["check"], "private_keyring_running")
        self.assertEqual(report["diagnostic_failures"], [{"stage": "keyring_log", "error_type": "PermissionError"}])
        self.assertIn("diagnostic-tail", report["service_diagnostics"]["bus_log"]["tail"])

    def test_workspace_cleanup_failure_keeps_original_service_failure(self):
        report, _, _, _, _ = self.exercise(path_bytes=107, workspace_failure=True)
        self.assertEqual(report["failure"]["check"], "private_keyring_running")
        self.assertEqual(report["cleanup_failures"], [{"stage": "workspace_exit", "type": "OSError"}])
        self.assertIn("diagnostic-tail", report["service_diagnostics"]["keyring_log"]["tail"])

    def test_thread_start_failure_closes_listener_without_blocking_shutdown(self):
        report, server, _, processes, _ = self.exercise(path_bytes=107, thread_failure=True)
        self.assertEqual(report["failure"]["type"], "RuntimeError")
        processes.assert_not_called()
        server.shutdown.assert_not_called()
        server.server_close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
