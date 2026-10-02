#!/usr/bin/env python3
"""Verify the six native packages and combine their per-target checksums."""
import hashlib
import os
from pathlib import Path
import shutil
import tarfile
import zipfile

TARGETS = (
    'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl',
    'x86_64-apple-darwin', 'aarch64-apple-darwin',
    'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc',
)
source, output = Path('release-artifacts'), Path('release-assets')
tag = os.environ['RELEASE_TAG']
if {path.name for path in source.iterdir()} != {'release-' + target for target in TARGETS}:
    raise SystemExit('Expected exactly six verified target artifacts')
output.mkdir()
entries = []
for target in sorted(TARGETS):
    windows = target.endswith('windows-msvc')
    extension = 'zip' if windows else 'tar.gz'
    name = f'jev-observer-{tag}-{target}.{extension}'
    directory = source / ('release-' + target)
    if {path.name for path in directory.iterdir()} != {name, 'SHA256SUMS'}:
        raise SystemExit(f'Unexpected files in {directory}')
    archive = directory / name
    entry = f'{hashlib.sha256(archive.read_bytes()).hexdigest()}  {name}\n'
    if (directory / 'SHA256SUMS').read_text() != entry:
        raise SystemExit(f'Checksum mismatch for {name}')
    if windows:
        with zipfile.ZipFile(archive) as package:
            members = package.infolist()
            if len(members) != 1 or members[0].filename != 'jev-observer.exe' or members[0].is_dir():
                raise SystemExit(f'Unexpected archive contents in {name}')
            if package.testzip() is not None:
                raise SystemExit(f'Corrupt ZIP: {name}')
    else:
        with tarfile.open(archive, 'r:gz') as package:
            members = package.getmembers()
            if len(members) != 1 or members[0].name != 'jev-observer' or not members[0].isreg() or members[0].mode != 0o755:
                raise SystemExit(f'Unexpected archive contents in {name}')
    shutil.copyfile(archive, output / name)
    entries.append(entry)
(output / 'SHA256SUMS').write_text(''.join(entries))
print('Verified six native archives and the aggregate SHA256SUMS manifest')
