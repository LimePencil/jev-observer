#!/usr/bin/env python3
"""Exercise dual-version fixture authentication without starting Observer."""
import base64
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest import mock
import urllib.error


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


compare = load("compare_dashboard", "compare-dashboard.py")
upgrade = load("test_upgrade", "test-upgrade.py")
CURRENT = "jo_access_" + "a" * 64
LEGACY = "jo_access_" + "b" * 64


class Connection:
    def __init__(self, accepted):
        self.accepted = "Basic " + base64.b64encode(("observer:" + accepted).encode()).decode()
        self.attempts = []

    def request(self, method, path, headers):
        assert (method, path) == ("GET", "/api/health")
        self.attempts.append(headers["Authorization"])

    def getresponse(self):
        accepted = self.attempts[-1] == self.accepted
        return SimpleNamespace(status=200 if accepted else 401,
                               read=lambda: b'{"persisted":0}' if accepted else b'Unauthorized')

    def close(self):
        pass


class TokenLayouts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.database = Path(self.temporary.name) / "history.sqlite"
        self.current = Path(str(self.database) + ".token")
        self.legacy = self.database.with_suffix(".access-token")

    def check_comparison(self, accepted, layout, attempts):
        connection = Connection(accepted)
        result = compare.authenticate_fixture(connection, self.database)
        self.assertIsNotNone(result)
        self.assertEqual(result[0], {"persisted": 0})
        self.assertEqual(result[1], connection.accepted)
        self.assertEqual(result[2], layout)
        self.assertEqual(len(connection.attempts), attempts)

    def test_current_binary_with_current_file(self):
        self.current.write_text(CURRENT)
        self.check_comparison(CURRENT, "current", 1)

    def test_old_binary_with_legacy_file(self):
        self.legacy.write_text(LEGACY)
        self.check_comparison(LEGACY, "legacy", 1)

    def test_current_binary_with_both_files(self):
        self.current.write_text(CURRENT)
        self.legacy.write_text(LEGACY)
        self.check_comparison(CURRENT, "current", 1)

    def test_rollback_with_both_files_authenticates_legacy(self):
        self.current.write_text(CURRENT)
        self.legacy.write_text(LEGACY)
        self.check_comparison(LEGACY, "legacy", 2)

    def test_wrong_tokens_do_not_authenticate(self):
        self.current.write_text(CURRENT)
        self.legacy.write_text(LEGACY)
        connection = Connection("jo_access_" + "c" * 64)
        self.assertIsNone(compare.authenticate_fixture(connection, self.database))
        self.assertEqual(len(connection.attempts), 2)

    def test_invalid_files_do_not_send_header_content(self):
        self.current.write_text(CURRENT + "\nextra")
        self.legacy.write_text(LEGACY)
        self.check_comparison(LEGACY, "legacy", 1)

    def start_upgrade(self, accepted, *, legacy=False):
        app = upgrade.Upgrade(SimpleNamespace())
        app.database = self.database
        app.directory = Path(self.temporary.name)
        app.upstream = "http://127.0.0.1:9/v1/systemone"

        def health(route):
            self.assertEqual(route, "/api/health")
            if app.token != accepted:
                raise urllib.error.URLError("Unauthorized")
            return {"persisted": 0}

        app.api = health
        try:
            with mock.patch.object(app, "stop"), mock.patch.object(upgrade.socket, "socket") as socket, \
                    mock.patch.object(upgrade.subprocess, "Popen") as process, \
                    mock.patch.object(upgrade.time, "sleep"):
                socket.return_value.__enter__.return_value.getsockname.return_value = ("127.0.0.1", 12345)
                process.return_value.poll.return_value = None
                app.start(Path("owned-fixture-binary"), legacy_token=legacy)
            return app.token_path
        finally:
            if app.log is not None:
                app.log.close()

    def test_upgrade_and_rollback_use_explicit_layouts_with_both_files(self):
        self.current.write_text(CURRENT)
        self.legacy.write_text(LEGACY)
        self.assertEqual(self.start_upgrade(CURRENT), self.current)
        self.assertEqual(self.start_upgrade(LEGACY, legacy=True), self.legacy)

    def test_candidate_never_falls_back_to_legacy_authentication(self):
        self.current.write_text(CURRENT)
        self.legacy.write_text(LEGACY)
        with self.assertRaisesRegex(AssertionError, "setup_startup_deadline"):
            self.start_upgrade(LEGACY)


if __name__ == "__main__":
    unittest.main()
