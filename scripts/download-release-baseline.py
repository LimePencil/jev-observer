#!/usr/bin/env python3
"""Download and checksum-verify the published 0.1.0 native upgrade baseline."""
import argparse
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=(
        "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-apple-darwin",
        "aarch64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"))
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    windows = args.target.endswith("windows-msvc")
    extension = "zip" if windows else "tar.gz"
    name = f"jev-observer-v0.1.0-{args.target}.{extension}"
    base = "https://github.com/LimePencil/jev-observer/releases/download/v0.1.0/"

    def download(asset):
        request = urllib.request.Request(base + asset, headers={"User-Agent": "Jev-Observer-upgrade-check"})
        with urllib.request.urlopen(request, timeout=120) as response:
            if not response.url.startswith("https://"):
                raise ValueError("Baseline download must remain HTTPS")
            return response.read()

    manifest = download("SHA256SUMS")
    entries = [line.split() for line in manifest.decode().splitlines() if line.split()[-1:] == [name]]
    archive = download(name)
    if len(entries) != 1 or entries[0] != [hashlib.sha256(archive).hexdigest(), name]:
        raise ValueError("Published baseline checksum mismatch")
    executable = "jev-observer.exe" if windows else "jev-observer"
    if windows:
        with zipfile.ZipFile(io.BytesIO(archive)) as package:
            if package.namelist() != [executable] or package.infolist()[0].is_dir():
                raise ValueError("Unexpected baseline archive contents")
            binary = package.read(executable)
    else:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as package:
            members = package.getmembers()
            if len(members) != 1 or members[0].name != executable or not members[0].isreg():
                raise ValueError("Unexpected baseline archive contents")
            binary = package.extractfile(members[0]).read()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    destination = args.output_dir / executable
    destination.write_bytes(binary)
    destination.chmod(0o755)
    (args.output_dir / "SHA256SUMS").write_bytes(manifest)
    print(f"Verified published 0.1.0 baseline: {args.target}; SHA256 {hashlib.sha256(binary).hexdigest()}")


if __name__ == "__main__":
    main()
