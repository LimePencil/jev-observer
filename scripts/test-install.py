#!/usr/bin/env python3
"""Local-only installer regression tests; no release downloads or credentials.

Run with `python3 scripts/test-install.py`. Platform detection is simulated with
uname stubs; these checks do not claim native macOS/ARM executable coverage.
"""

import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "install.sh"
TARGETS = {
    ("Linux", "x86_64"): "x86_64-unknown-linux-musl",
    ("Linux", "aarch64"): "aarch64-unknown-linux-musl",
    ("Darwin", "x86_64"): "x86_64-apple-darwin",
    ("Darwin", "arm64"): "aarch64-apple-darwin",
}
VERSION = "1.2.3"


def executable(version=VERSION, exit_code=0):
    return ("#!/bin/sh\n"
            "[ \"$#\" -eq 1 ] && [ \"$1\" = --version ] || exit 64\n"
            f"printf '%s\\n' 'jev-observer {version}'\nexit {exit_code}\n").encode()


def archive(entries=None, version=VERSION):
    output = io.BytesIO()
    if entries is None:
        entries = [("jev-observer", executable(version), tarfile.REGTYPE, "", 0o755)]
    with tarfile.open(fileobj=output, mode="w:gz") as bundle:
        for name, content, kind, link, mode in entries:
            item = tarfile.TarInfo(name)
            item.type, item.linkname, item.mode = kind, link, mode
            item.size = len(content) if kind == tarfile.REGTYPE else 0
            bundle.addfile(item, io.BytesIO(content) if kind == tarfile.REGTYPE else None)
    return output.getvalue()


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.requests.append(self.path)
        status, body = self.server.routes.get(self.path, (404, b"Fixture asset not found"))
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *args):
        pass


class InstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not INSTALLER.is_file():
            raise RuntimeError(f"Installer is missing: {INSTALLER}")
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
        cls.server.routes, cls.server.requests = {}, []
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=5)

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="observer-installer-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.home = self.directory / "home"
        self.home.mkdir()
        self.download_tmp = self.directory / "temporary files"
        self.download_tmp.mkdir()
        self.install_dir = self.directory / "tools with spaces" / "bin directory"
        self.stub_dir = self.directory / "stubs"
        self.stub_dir.mkdir()
        self.fixture_dir = self.directory / "releases"
        self.fixture_dir.mkdir()
        self.server.routes.clear()
        self.server.requests.clear()
        self.environment = {
            "HOME": str(self.home), "TMPDIR": str(self.download_tmp), "PATH": str(self.stub_dir) + os.pathsep + os.defpath,
            "LC_ALL": "C", "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
            "JEV_OBSERVER_RELEASE_BASE_URL": f"http://127.0.0.1:{self.server.server_port}/releases",
            "JEV_OBSERVER_ALLOW_INSECURE_HTTP": "1",
        }
        self.platform()
        self.publish()

    def write_executable(self, path, body):
        path.write_text(body)
        path.chmod(0o755)

    def platform(self, system="Linux", machine="x86_64"):
        self.write_executable(self.stub_dir / "uname", "#!/bin/sh\ncase \"$1\" in\n"
                              f"-s) printf '%s\\n' '{system}' ;;\n-m) printf '%s\\n' '{machine}' ;;\n"
                              "*) exit 2 ;;\nesac\n")

    def serve(self, relative, body, status=200):
        self.server.routes["/releases/" + relative] = (status, body)
        path = self.fixture_dir / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)

    def publish(self, version=VERSION, bundle=None, checksum=None, targets=None):
        bundle = archive(version=version) if bundle is None else bundle
        checksum = hashlib.sha256(bundle).hexdigest() if checksum is None else checksum
        lines = []
        for target in targets or TARGETS.values():
            name = f"jev-observer-v{version}-{target}.tar.gz"
            self.serve(f"download/v{version}/{name}", bundle)
            lines.append(f"{checksum}  {name}\n")
        manifest = "".join(lines).encode()
        self.serve("latest/download/SHA256SUMS", manifest)
        self.serve(f"download/v{version}/SHA256SUMS", manifest)

    def run_installer(self, *arguments, overrides=None, default_directory=False):
        environment = self.environment.copy()
        for key, value in (overrides or {}).items():
            if value is None:
                environment.pop(key, None)
            else:
                environment[key] = value
        command = ["/bin/sh", str(INSTALLER)]
        if not default_directory:
            command.extend(["--install-dir", str(self.install_dir)])
        return subprocess.run(command + list(arguments), cwd=self.directory, env=environment,
                              text=True, capture_output=True, timeout=20)

    def assert_success(self, result, version=VERSION, directory=None):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        directory = directory or self.install_dir
        binary = directory / "jev-observer"
        self.assertTrue(binary.is_file())
        self.assertTrue(os.access(binary, os.X_OK))
        probe = subprocess.run([str(binary), "--version"], capture_output=True, text=True, timeout=5)
        self.assertEqual(probe.returncode, 0)
        self.assertEqual(probe.stdout.strip(), "jev-observer " + version)
        self.assertEqual(sorted(path.name for path in directory.iterdir()), ["jev-observer"], "Installer left temporary files behind")

    def seed_existing(self):
        self.install_dir.mkdir(parents=True, exist_ok=True)
        binary = self.install_dir / "jev-observer"
        binary.write_bytes(executable("0.0.1"))
        binary.chmod(0o751)
        return binary.read_bytes(), stat.S_IMODE(binary.stat().st_mode)

    def assert_preserved(self, previous):
        binary = self.install_dir / "jev-observer"
        self.assertEqual((binary.read_bytes(), stat.S_IMODE(binary.stat().st_mode)), previous)
        self.assertEqual(sorted(path.name for path in self.install_dir.iterdir()), ["jev-observer"])

    def assert_failure(self, result, message_pattern):
        self.assertNotEqual(result.returncode, 0, "Installer unexpectedly succeeded")
        self.assertRegex((result.stdout + result.stderr).lower(), message_pattern)

    def limited_path(self, omitted):
        directory = self.directory / "limited-path"
        directory.mkdir(exist_ok=True)
        commands = "awk tar gzip mktemp mkdir chmod mv rm cmp wc cat sha256sum shasum curl wget sed grep tr cut head tail sort basename dirname env date sleep cp ls readlink id printf".split()
        for command in commands:
            source = shutil.which(command)
            if command not in omitted and source:
                (directory / command).symlink_to(source)
        (directory / "uname").symlink_to(self.stub_dir / "uname")
        return str(directory)

    def gh_stub(self, fail_download=False):
        log = self.directory / "gh-calls.jsonl"
        source = f'''#!{sys.executable}
import json, os, pathlib, shutil, sys
arguments = sys.argv[1:]
with open(os.environ['FIXTURE_GH_LOG'], 'a') as stream:
    stream.write(json.dumps(arguments) + '\\n')
if arguments[:2] == ['auth', 'status']:
    sys.exit(0)
if arguments[:2] != ['release', 'download']:
    sys.exit(65)
if {fail_download!r}:
    print('do-not-echo-private-download-log', file=sys.stderr)
    sys.exit(1)
name = arguments[arguments.index('--pattern') + 1]
destination = pathlib.Path(arguments[arguments.index('--dir') + 1])
tag = next((item for item in arguments[2:] if item.startswith('v') and '/' not in item), None)
relative = pathlib.Path('download') / tag / name if tag else pathlib.Path('latest/download') / name
source = pathlib.Path(os.environ['FIXTURE_GH_ROOT']) / relative
if not source.is_file():
    sys.exit(66)
destination.mkdir(parents=True, exist_ok=True)
shutil.copyfile(source, destination / name)
'''
        self.write_executable(self.stub_dir / "gh", source)
        return log, {"JEV_OBSERVER_RELEASE_BASE_URL": None, "JEV_OBSERVER_ALLOW_INSECURE_HTTP": None,
                     "FIXTURE_GH_ROOT": str(self.fixture_dir), "FIXTURE_GH_LOG": str(log), "GH_HOST": "unexpected.invalid"}

    def test_verified_install_rerun_and_upgrade_in_path_with_spaces(self):
        self.assert_success(self.run_installer())
        self.assert_success(self.run_installer())
        self.publish("1.2.4")
        self.assert_success(self.run_installer(), version="1.2.4")

    def test_default_install_directory_is_under_home(self):
        self.assert_success(self.run_installer(default_directory=True), directory=self.home / ".local/bin")

    def test_printed_path_commands_quote_apostrophes_and_shell_metacharacters(self):
        self.install_dir = self.directory / "bin' quoted; $(touch should-not-execute)"
        result = self.run_installer()
        self.assert_success(result)
        command = next(line.strip() for line in result.stdout.splitlines() if line.strip().startswith("export PATH="))
        probe = subprocess.run(["/bin/sh", "-c", command + '\nprintf "%s\\n" "$PATH"'],
                               cwd=self.directory, env=self.environment, text=True, capture_output=True, timeout=5)
        self.assertEqual(probe.returncode, 0, probe.stderr)
        # macOS resolves /var to /private/var when the installer canonicalizes
        # its destination. Compare the actual directory, retaining literal quotes.
        self.assertEqual(probe.stdout.strip(), str(self.install_dir.resolve()) + os.pathsep + self.environment["PATH"])
        self.assertFalse((self.directory / "should-not-execute").exists())

    def test_checksum_verification_handles_backslashes_in_temporary_directory(self):
        temporary = self.directory / "temporary\\files"
        temporary.mkdir()
        self.assert_success(self.run_installer(overrides={"TMPDIR": str(temporary)}))
        if shutil.which("shasum"):
            self.assert_success(self.run_installer(overrides={"TMPDIR": str(temporary), "PATH": self.limited_path({"sha256sum"})}))

    def test_pinned_version_accepts_optional_v_and_never_fetches_latest(self):
        for version in (VERSION, "v" + VERSION):
            with self.subTest(version=version):
                self.server.requests.clear()
                self.assert_success(self.run_installer("--version", version))
                self.assertTrue(self.server.requests)
                self.assertTrue(all("/download/v1.2.3/" in path for path in self.server.requests))

    def test_latest_manifest_pins_archive_to_its_version(self):
        self.assert_success(self.run_installer())
        self.assertEqual(self.server.requests[0], "/releases/latest/download/SHA256SUMS")
        self.assertIn(f"/releases/download/v{VERSION}/jev-observer-v{VERSION}-x86_64-unknown-linux-musl.tar.gz", self.server.requests)
        self.assertFalse(any("latest/download/jev-observer" in path for path in self.server.requests))

    def test_all_supported_platform_asset_names(self):
        for (system, machine), target in TARGETS.items():
            with self.subTest(system=system, machine=machine):
                self.platform(system, machine)
                self.server.requests.clear()
                self.assert_success(self.run_installer())
                self.assertTrue(any(path.endswith(target + ".tar.gz") for path in self.server.requests))

    def test_bad_checksum_preserves_existing_binary_and_permissions(self):
        previous = self.seed_existing()
        self.publish(checksum="0" * 64)
        self.assert_failure(self.run_installer(), "checksum|sha256|hash")
        self.assert_preserved(previous)

    def test_wrong_or_failing_binary_version_preserves_existing_install(self):
        previous = self.seed_existing()
        for content in (executable("9.9.9"), executable(VERSION, exit_code=17), b""):
            with self.subTest(content=content):
                self.publish(bundle=archive([("jev-observer", content, tarfile.REGTYPE, "", 0o755)]))
                self.assert_failure(self.run_installer(), "version|execut|run")
                self.assert_preserved(previous)

    def test_malformed_archive_is_rejected_before_install(self):
        previous = self.seed_existing()
        self.publish(bundle=b"This is not a gzip tar archive")
        self.assert_failure(self.run_installer(), "archive|tar|extract")
        self.assert_preserved(previous)

    def test_unsafe_archive_entries_are_rejected_without_filesystem_escape(self):
        previous = self.seed_existing()
        link_target = self.directory / "untouched-link-target"
        link_target.write_bytes(executable("0.0.2"))
        link_target.chmod(0o751)
        target_before = (link_target.read_bytes(), stat.S_IMODE(link_target.stat().st_mode))
        normal = ("jev-observer", executable(), tarfile.REGTYPE, "", 0o755)
        cases = {
            "traversal": [normal, ("../escaped", b"bad", tarfile.REGTYPE, "", 0o644)],
            "absolute": [normal, (str(self.directory / "escaped-absolute"), b"bad", tarfile.REGTYPE, "", 0o644)],
            "symlink": [("jev-observer", b"", tarfile.SYMTYPE, str(link_target), 0o755)],
            "hardlink": [("jev-observer", b"", tarfile.LNKTYPE, str(link_target), 0o755)],
            "directory": [("jev-observer", b"", tarfile.DIRTYPE, "", 0o755)],
            "duplicate": [normal, normal],
            "nested": [("bin/jev-observer", executable(), tarfile.REGTYPE, "", 0o755)],
            "not-executable": [("jev-observer", executable(), tarfile.REGTYPE, "", 0o644)],
        }
        for name, entries in cases.items():
            with self.subTest(archive=name):
                self.publish(bundle=archive(entries))
                self.assert_failure(self.run_installer(), "archive|regular|single|entry|entries|link|tar")
                self.assert_preserved(previous)
                self.assertFalse(list(self.directory.rglob("escaped*")))
                self.assertEqual((link_target.read_bytes(), stat.S_IMODE(link_target.stat().st_mode)), target_before)

    def test_unsupported_platforms_fail_before_network_access(self):
        for system, machine in (("Plan9", "x86_64"), ("Linux", "mips")):
            with self.subTest(system=system, machine=machine):
                self.platform(system, machine)
                self.server.requests.clear()
                self.assert_failure(self.run_installer(), "unsupported|support")
                self.assertEqual(self.server.requests, [])

    def test_invalid_version_and_arguments_fail_before_network_access(self):
        cases = [("--version", value) for value in ("", "v", "1.2", "../escape", "1.2.3/path", "1.2.3;echo bad")]
        cases.extend([("--unknown",), ("--version",)])
        for arguments in cases:
            with self.subTest(arguments=arguments):
                self.server.requests.clear()
                self.assert_failure(self.run_installer(*arguments), "version|argument|usage|unknown|option|required")
                self.assertEqual(self.server.requests, [])

    def test_plain_http_requires_explicit_fixture_opt_in(self):
        self.assert_failure(self.run_installer(overrides={"JEV_OBSERVER_ALLOW_INSECURE_HTTP": None}), "https|insecure|http")
        self.assertEqual(self.server.requests, [])

    def test_missing_manifest_and_asset_http_errors_preserve_existing_install(self):
        for which, status in (("manifest", 404), ("manifest", 503), ("asset", 404)):
            with self.subTest(which=which, status=status):
                self.publish()
                previous = self.seed_existing()
                path = "/releases/latest/download/SHA256SUMS" if which == "manifest" else f"/releases/download/v{VERSION}/jev-observer-v{VERSION}-x86_64-unknown-linux-musl.tar.gz"
                self.server.routes[path] = (status, b"Fixture download failure")
                self.assert_failure(self.run_installer(), "download|fetch|http|release")
                self.assert_preserved(previous)

    def test_missing_target_asset_in_manifest_is_clear(self):
        previous = self.seed_existing()
        self.publish(targets=["aarch64-unknown-linux-musl"])
        self.assert_failure(self.run_installer(), "target|asset|archive|release")
        self.assert_preserved(previous)

    def test_shasum_fallback(self):
        if not shutil.which("shasum"):
            self.skipTest("shasum is unavailable on this test host")
        self.assert_success(self.run_installer(overrides={"PATH": self.limited_path({"sha256sum"})}))

    def test_wget_fallback(self):
        if not shutil.which("wget"):
            self.skipTest("wget is unavailable on this test host")
        self.assert_success(self.run_installer(overrides={"PATH": self.limited_path({"curl"})}))

    def test_authenticated_gh_latest_manifest_then_pinned_archive(self):
        log, overrides = self.gh_stub()
        self.assert_success(self.run_installer(overrides=overrides))
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertEqual(calls[0][:2], ["auth", "status"])
        self.assertIn("--active", calls[0])
        self.assertEqual(calls[0][calls[0].index("--hostname") + 1], "github.com")
        downloads = [call for call in calls if call[:2] == ["release", "download"]]
        self.assertEqual(len(downloads), 2)
        self.assertNotIn("v" + VERSION, downloads[0])
        self.assertIn("v" + VERSION, downloads[1])
        for call in downloads:
            self.assertEqual(call[call.index("--repo") + 1], "github.com/LimePencil/jev-observer")
        self.assertEqual(self.server.requests, [])

    def test_gh_failure_does_not_echo_private_download_logs(self):
        previous = self.seed_existing()
        _, overrides = self.gh_stub(fail_download=True)
        result = self.run_installer(overrides=overrides)
        self.assert_failure(result, "download|release|github")
        self.assertNotIn("do-not-echo-private-download-log", result.stdout + result.stderr)
        self.assert_preserved(previous)
        self.assertEqual(self.server.requests, [])

    def test_custom_mirror_does_not_probe_or_use_github_credentials(self):
        log, overrides = self.gh_stub()
        overrides["JEV_OBSERVER_RELEASE_BASE_URL"] = self.environment["JEV_OBSERVER_RELEASE_BASE_URL"]
        overrides["JEV_OBSERVER_ALLOW_INSECURE_HTTP"] = "1"
        self.assert_success(self.run_installer(overrides=overrides))
        self.assertFalse(log.exists(), "A custom mirror unexpectedly invoked gh")
        self.assertTrue(self.server.requests)


if __name__ == "__main__":
    unittest.main(verbosity=2)
