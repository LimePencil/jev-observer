#!/usr/bin/env python3
import argparse
import base64
import gzip
import hashlib
import http.client
import io
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import zipfile
import signal
from urllib.parse import quote

root = Path(__file__).resolve().parent.parent
targets = {
    "x86_64-unknown-linux-musl": ("Linux", "x86_64", 62),
    "aarch64-unknown-linux-musl": ("Linux", "aarch64", 183),
    "x86_64-apple-darwin": ("Darwin", "x86_64", 0x01000007),
    "aarch64-apple-darwin": ("Darwin", "aarch64", 0x0100000C),
    "x86_64-pc-windows-msvc": ("Windows", "x86_64", 0x8664),
    "aarch64-pc-windows-msvc": ("Windows", "aarch64", 0xAA64),
}
parser = argparse.ArgumentParser(
    prog="scripts/package-release.sh",
    description="Verify and package a native Observer release with its embedded dashboard.",
    epilog="Build ui/dist first, then cargo build --release --locked --target TARGET. "
           "The archive contains exactly one root executable: jev-observer. "
           "SHA256SUMS is regenerated for this version's supported archives in the output directory.")
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--target", choices=targets, required=True)
parser.add_argument("--version", help="Cargo package version, optionally prefixed with v; defaults to Cargo.toml")
parser.add_argument("--output-dir", type=Path, help="Defaults to .jev-observer/releases/vVERSION")
args = parser.parse_args()
version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
if args.version is not None and args.version.removeprefix("v") != version:
    parser.error(f"--version must match Cargo.toml ({version})")
host_machine = platform.machine().lower()
host_arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(host_machine, host_machine)
system, architecture, machine = targets[args.target]
if (platform.system(), host_arch) != (system, architecture):
    parser.error("Package on the target's native operating system and architecture so its executable can be tested")
binary = args.binary.resolve()
if not binary.is_file() or not os.access(binary, os.X_OK):
    parser.error(f"Missing executable: {binary}")
ui = root / "ui/dist"
if not (ui / "index.html").is_file() or not list((ui / "assets").glob("*.js")) or not list((ui / "assets").glob("*.css")):
    parser.error("Missing dashboard build: run npm ci --prefix ui && npm run build --prefix ui before building Rust")
license_path = ui / "licenses/jev-observer-MIT.txt"
if not license_path.is_file() or license_path.read_bytes() != (root / "LICENSE").read_bytes():
    parser.error("Dashboard build must contain the current root MIT license; rebuild ui/dist")
subprocess.run([sys.executable, str(root / "scripts/generate-license-notices.py"), "--check"],
               cwd=root, check=True)
for name in ("THIRD-PARTY-NOTICES.txt", "third-party-inventory.json", "rust-standard-library.html", "musl-COPYRIGHT.txt"):
    if (ui / "licenses" / name).read_bytes() != (root / "ui/public/licenses" / name).read_bytes():
        parser.error("Dashboard dependency notices are stale; rebuild ui/dist")
output = (args.output_dir or root / ".jev-observer/releases" / f"v{version}").resolve()
extension = "zip" if system == "Windows" else "tar.gz"
executable_name = "jev-observer.exe" if system == "Windows" else "jev-observer"
archive_name = f"jev-observer-v{version}-{args.target}.{extension}"
archive_path = output / archive_name
if archive_path.exists():
    parser.error(f"Refusing to replace an existing archive: {archive_path}")


def inspect_binary(data):
    if system == "Windows":
        if data[:2] != b"MZ":
            raise RuntimeError("Windows executable must have a DOS header")
        offset = struct.unpack_from("<I", data, 60)[0]
        if data[offset:offset + 4] != b"PE\0\0" or struct.unpack_from("<H", data, offset + 4)[0] != machine:
            raise RuntimeError("Executable is not a PE binary for the requested architecture")
        # Runner images contain VC redistributables; smoke tests alone cannot
        # establish that a package runs without them on a fresh Windows install.
        optional = offset + 24
        if struct.unpack_from("<H", data, optional)[0] != 0x20B:
            raise RuntimeError("Windows package must contain a PE32+ executable")
        count = struct.unpack_from("<H", data, offset + 6)[0]
        section_offset = optional + struct.unpack_from("<H", data, offset + 20)[0]
        sections = [struct.unpack_from("<IIII", data, section_offset + index * 40 + 8)
                    for index in range(count)]

        def file_offset(rva):
            for virtual_size, virtual_address, raw_size, raw_offset in sections:
                if virtual_address <= rva < virtual_address + max(virtual_size, raw_size):
                    result = raw_offset + rva - virtual_address
                    if result < len(data):
                        return result
            raise RuntimeError("Invalid PE import address")

        imports = []
        import_rva = struct.unpack_from("<I", data, optional + 112 + 8)[0]
        if import_rva:
            cursor = file_offset(import_rva)
            while any(data[cursor:cursor + 20]):
                name = file_offset(struct.unpack_from("<I", data, cursor + 12)[0])
                end = data.index(b"\0", name)
                imports.append(data[name:end].decode("ascii").lower())
                cursor += 20
        if any(name.startswith(("vcruntime", "msvcp", "concrt", "libcrypto", "libssl", "sqlite3"))
               for name in imports):
            raise RuntimeError(f"Windows release depends on a non-bundled runtime: {imports}")
        print("Verified Windows DLL imports: " + ", ".join(imports))
        return
    if system == "Darwin":
        if data[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", data, 4)[0] != machine:
            raise RuntimeError("Executable is not a 64-bit Mach-O binary for the requested architecture")
        return
    if data[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", data, 18)[0] != machine:
        raise RuntimeError("Executable is not a 64-bit ELF binary for the requested architecture")
    start = struct.unpack_from("<Q", data, 32)[0]
    stride, count = struct.unpack_from("<HH", data, 54)
    for index in range(count):
        header = start + index * stride
        kind = struct.unpack_from("<I", data, header)[0]
        if kind == 3:  # PT_INTERP: a dynamic loader defeats a standalone musl release.
            raise RuntimeError("Linux release must be static musl: executable contains a dynamic interpreter")
        if kind == 2:  # Static PIE can contain PT_DYNAMIC, but must not need libraries.
            offset = struct.unpack_from("<Q", data, header + 8)[0]
            size = struct.unpack_from("<Q", data, header + 32)[0]
            for entry in range(offset, offset + size, 16):
                tag = struct.unpack_from("<q", data, entry)[0]
                if tag == 0:
                    break
                if tag == 1:
                    raise RuntimeError("Linux release must not depend on shared libraries")


def smoke_test(executable, directory, demo=False):
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ("typesafe_api_key", "http_proxy", "https_proxy", "all_proxy")}
    environment["NO_PROXY"] = "127.0.0.1,localhost"
    environment["JEV_OBSERVER_DB_KEY"] = os.urandom(32).hex()
    found_version = subprocess.check_output([str(executable), "--version"], env=environment, text=True, timeout=10).strip()
    if found_version != f"jev-observer {version}":
        raise RuntimeError(f"Wrong binary version: {found_version!r}")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    log_path = directory / "smoke.log"
    token_path = directory / ("smoke.demo.access-token" if demo else "smoke.access-token")
    command = [str(executable), "--port", str(port), "--db", str(directory / "smoke.sqlite"),
               "--upstream", "http://127.0.0.1:9/v1/systemone"]
    if demo:
        command.append("--demo")
    with log_path.open("wb") as log:
        process = subprocess.Popen(command, cwd=directory, env=environment,
                                   stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                   creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if system == "Windows" else 0)
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            deadline = time.monotonic() + 20
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Release executable exited during startup: " + log_path.read_text(errors="replace")[-2000:])
                try:
                    token = token_path.read_text()
                    authorization = "Basic " + base64.b64encode(f"observer:{token}".encode()).decode()
                    connection.request("GET", "/api/health", headers={"Authorization": authorization})
                    response = connection.getresponse()
                    if response.status != 200 or json.loads(response.read()).get("forwarded") != 0:
                        raise RuntimeError("Unexpected release health response")
                    break
                except (OSError, http.client.HTTPException):
                    connection.close()
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Release executable did not become healthy within 20 seconds")
                    time.sleep(0.1)
            files = sorted(file for file in ui.rglob("*") if file.is_file())
            for file in files:
                relative = file.relative_to(ui).as_posix()
                connection.request("GET", "/" if relative == "index.html" else "/" + quote(relative), headers={"Authorization": authorization})
                response = connection.getresponse()
                actual = response.read()
                if response.status != 200 or actual != file.read_bytes():
                    raise RuntimeError(f"Embedded dashboard differs from ui/dist/{relative}; rebuild Rust after the UI")
            connection.request("GET", "/api/dashboard?window=all", headers={"Authorization": authorization})
            response = connection.getresponse()
            dashboard = json.loads(response.read())
            expected = 720 if demo else 0
            if response.status != 200 or dashboard["summary"]["request_count"] != expected:
                raise RuntimeError(f"Release dashboard did not report {expected} expected records")
            print(f"Verified {found_version}, health, {expected} records, and {len(files)} embedded UI files")
        finally:
            connection.close()
            if process.poll() is None:
                if system == "Windows":
                    process.send_signal(signal.CTRL_BREAK_EVENT)
                else:
                    process.terminate()
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                    raise RuntimeError("Release executable did not shut down cleanly")
        if process.returncode != 0:
            raise RuntimeError(f"Release executable exited with status {process.returncode}")


try:
    with tempfile.TemporaryDirectory(prefix="jev-observer-release-") as temporary:
        directory = Path(temporary) / ".jev-observer"
        directory.mkdir()
        executable = directory / executable_name
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        data = executable.read_bytes()
        inspect_binary(data)
        smoke_test(executable, directory)
        smoke_test(executable, directory, demo=True)
        staged_archive = directory / archive_name
        if system == "Windows":
            with zipfile.ZipFile(staged_archive, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                entry = zipfile.ZipInfo(executable_name, date_time=(1980, 1, 1, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = 0o100755 << 16
                archive.writestr(entry, data)
        else:
            with staged_archive.open("wb") as stream, gzip.GzipFile(filename="", mode="wb", fileobj=stream, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                    entry = tarfile.TarInfo(executable_name)
                    entry.size = len(data)
                    entry.mode = 0o755
                    archive.addfile(entry, io.BytesIO(data))
        output.mkdir(parents=True, exist_ok=True)
        # Copy into the destination filesystem before the atomic rename.
        with tempfile.NamedTemporaryFile(dir=output, prefix=".archive-", delete=False) as stream:
            staging_path = Path(stream.name)
            with staged_archive.open("rb") as source:
                shutil.copyfileobj(source, stream)
        try:
            staging_path.replace(archive_path)
            archive_path.chmod(0o644)
        finally:
            staging_path.unlink(missing_ok=True)
        entries = []
        for target in sorted(targets):
            suffix = "zip" if targets[target][0] == "Windows" else "tar.gz"
            asset = output / f"jev-observer-v{version}-{target}.{suffix}"
            if asset.is_file():
                entries.append(f"{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n")
        with tempfile.NamedTemporaryFile(mode="w", dir=output, prefix=".checksums-", delete=False) as stream:
            checksum_path = Path(stream.name)
            stream.writelines(entries)
        try:
            checksum_path.replace(output / "SHA256SUMS")
            (output / "SHA256SUMS").chmod(0o644)
        finally:
            checksum_path.unlink(missing_ok=True)
        print(f"Packaged {archive_path}")
        print(f"Binary SHA256: {hashlib.sha256(data).hexdigest()}")
        print(f"Checksums: {output / 'SHA256SUMS'}")
except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, struct.error) as error:
    print(f"Packaging failed: {error}", file=sys.stderr)
    sys.exit(1)
