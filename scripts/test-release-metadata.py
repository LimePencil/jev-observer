#!/usr/bin/env python3
"""Regression checks for release gates and prerelease publication policy."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("check-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseMetadata(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "ui").mkdir()
        (self.root / "docs/releases").mkdir(parents=True)

    def fixture(self, version="0.2.0"):
        (self.root / "Cargo.toml").write_text(f'[package]\nname="jev-observer"\nversion="{version}"\n')
        (self.root / "Cargo.lock").write_text(f'[[package]]\nname="jev-observer"\nversion="{version}"\n')
        (self.root / "ui/package.json").write_text(json.dumps({"version": version}))
        (self.root / "ui/package-lock.json").write_text(json.dumps({"version": version, "packages": {"": {"version": version}}}))
        (self.root / "docs/releases" / (version + ".md")).write_text("Reviewed release notes\n")

    def test_stable_and_prerelease_policy(self):
        for version, prerelease in (("0.2.0", "false"), ("0.2.0-rc.1", "true"), ("0.2.0+build-one", "false")):
            self.fixture(version)
            result = release.metadata(self.root, "refs/tags/v" + version)
            self.assertEqual(result["prerelease"], prerelease)
            self.assertNotEqual(result["latest"], prerelease)

    def test_wrong_tag_is_rejected(self):
        self.fixture()
        with self.assertRaisesRegex(ValueError, "tag"):
            release.metadata(self.root, "refs/tags/v0.1.0")

    def test_numeric_prerelease_leading_zeros_are_rejected(self):
        for version in ("0.2.0-01", "0.2.0-rc.01"):
            self.fixture(version)
            with self.assertRaisesRegex(ValueError, "leading zeros"):
                release.metadata(self.root)

    def test_missing_notes_are_rejected_for_manual_dispatch(self):
        self.fixture()
        (self.root / "docs/releases/0.2.0.md").unlink()
        with self.assertRaisesRegex(ValueError, "notes"):
            release.metadata(self.root, "refs/heads/main")

    def test_stale_cargo_lock_is_rejected(self):
        self.fixture()
        path = self.root / "Cargo.lock"
        path.write_text(path.read_text().replace("0.2.0", "0.1.0"))
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            release.metadata(self.root)

    def test_stale_ui_lock_is_rejected(self):
        self.fixture()
        path = self.root / "ui/package-lock.json"
        lock = json.loads(path.read_text())
        lock["packages"][""]["version"] = "0.1.0"
        path.write_text(json.dumps(lock))
        with self.assertRaisesRegex(ValueError, "UI"):
            release.metadata(self.root)


if __name__ == "__main__":
    unittest.main(verbosity=2)
