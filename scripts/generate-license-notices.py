#!/usr/bin/env python3
"""Generate or check the notices bundled with the single-file release.

Generation needs cargo-about 0.9.2, fetched Cargo sources, npm ci in ui/, and
the Rust documentation component. Check mode uses only the committed files.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "ui/public/licenses"
parser = argparse.ArgumentParser()
parser.add_argument("--check", action="store_true")
parser.add_argument("--cargo-about", default="cargo-about")
args = parser.parse_args()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inputs():
    return {name: digest((ROOT / name).read_bytes()) for name in
            ("Cargo.lock", "ui/package-lock.json", "about.toml", "licenses/npm-supplemental.json")}


if args.check:
    inventory = json.loads((OUTPUT / "third-party-inventory.json").read_text())
    if inventory["inputs"] != inputs():
        raise SystemExit("Dependency locks or license policy changed; regenerate license notices.")
    for name, expected in inventory["outputs"].items():
        if digest((OUTPUT / name).read_bytes()) != expected:
            raise SystemExit(f"License notice changed: {name}; regenerate notices.")
    required = {"sqlcipher", "openssl", "rust-standard-library", "musl"}
    if not required.issubset({entry["name"] for entry in inventory["bundled_components"]}):
        raise SystemExit("Missing bundled native component notices.")
    if not inventory["rust_packages"] or not inventory["npm_packages"]:
        raise SystemExit("Missing dependency license inventory.")
    standard_library = next(entry for entry in inventory["bundled_components"]
                            if entry["name"] == "rust-standard-library")
    if subprocess.check_output(["rustc", "--version"], text=True).strip() != standard_library["source_version"]:
        raise SystemExit("Rust toolchain changed; regenerate standard-library notices before release.")
    for extra in json.loads((ROOT / "licenses/npm-supplemental.json").read_text()).values():
        if digest((ROOT / extra["path"]).read_bytes()) != extra["sha256"]:
            raise SystemExit("Pinned supplemental license text changed.")
    print("Bundled dependency notices match the locked source and notice hashes.")
    raise SystemExit(0)

version = subprocess.check_output([args.cargo_about, "--version"], text=True).strip()
if version != "cargo-about 0.9.2":
    raise SystemExit("Use cargo-about 0.9.2 to regenerate the committed inventory.")
model = json.loads(subprocess.check_output([
    args.cargo_about, "generate", "--locked", "--fail", "--format", "json"], cwd=ROOT))
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT))
packages = {p["name"]: p for p in metadata["packages"]}
inventory = {"generator": "cargo-about 0.9.2 and scripts/generate-license-notices.py",
             "inputs": inputs(), "rust_packages": [], "npm_packages": [],
             "bundled_components": [], "outputs": {}}
sections = ["Jev Observer — third-party notices\n\n"
            "Observer's own code is MIT-licensed. The following notices apply to dependencies.\n"
            "The inventory covers all six release targets and their build dependencies;\n"
            "individual executables use the components for their platform.\n"]
covered = set()
for group in model["licenses"]:
    users = sorted({(user["crate"]["name"], user["crate"]["version"])
                    for user in group["used_by"] if user["crate"]["name"] != "jev-observer"})
    if not users:
        continue
    covered.update(users)
    sections.append("\n" + "=" * 72 + "\n" + group["id"] + "\n" +
                    ", ".join(name + " " + ver for name, ver in users) + "\n\n" + group["text"])
for entry in model["crates"]:
    package = entry["package"]
    if package["name"] == "jev-observer":
        continue
    identity = (package["name"], package["version"])
    if identity not in covered:
        raise SystemExit(f"No notice gathered for Rust dependency: {identity}")
    inventory["rust_packages"].append({"name": identity[0], "version": identity[1],
                                        "declared_license": package["license"]})

lock = json.loads((ROOT / "ui/package-lock.json").read_text())
supplemental = json.loads((ROOT / "licenses/npm-supplemental.json").read_text())
for name, info in sorted(lock["packages"].items()):
    if not name or info.get("dev"):
        continue
    directory = ROOT / "ui" / name
    manifest = json.loads((directory / "package.json").read_text())
    if manifest["version"] != info["version"]:
        raise SystemExit(f"Installed npm dependency differs from the lock: {manifest['name']}")
    notice_files = sorted(p for p in directory.iterdir() if p.is_file() and
                          p.name.upper().startswith(("LICENSE", "LICENCE", "NOTICE", "COPYING", "COPYRIGHT")))
    if not notice_files:
        extra = supplemental.get(manifest["name"] + "@" + manifest["version"])
        if not extra:
            raise SystemExit(f"No source license file packaged by npm dependency: {manifest['name']}")
        source = ROOT / extra["path"]
        if digest(source.read_bytes()) != extra["sha256"]:
            raise SystemExit(f"Pinned supplemental license changed: {source.name}")
        notice_files = [source]
    texts = [p.read_text() for p in notice_files]
    sections.append("\n" + "=" * 72 + "\n" + manifest["name"] + " " +
                    manifest["version"] + "\n\n" + "\n\n".join(texts))
    inventory["npm_packages"].append({"name": manifest["name"], "version": manifest["version"],
                                       "declared_license": manifest.get("license"),
                                       "notice_files": [p.name for p in notice_files],
                                       "supplemental_source": supplemental.get(manifest["name"] + "@" + manifest["version"])})

for crate, name, relative in (("libsqlite3-sys", "sqlcipher", "sqlcipher/LICENSE"),
                              ("openssl-src", "openssl", "openssl/LICENSE.txt")):
    package = packages[crate]
    notice = (Path(package["manifest_path"]).parent / relative).read_text()
    sections.append("\n" + "=" * 72 + "\n" + name + " bundled by " + crate + " " +
                    package["version"] + "\n\n" + notice)
    inventory["bundled_components"].append({"name": name, "source_crate": crate,
                                            "source_version": package["version"],
                                            "notice_sha256": digest(notice.encode())})

sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
rust_notice = sysroot / "share/doc/rust/COPYRIGHT-library.html"
if not rust_notice.is_file():
    raise SystemExit("Install the rust-docs component before regenerating standard-library notices.")
OUTPUT.mkdir(parents=True, exist_ok=True)
(OUTPUT / "rust-standard-library.html").write_bytes(rust_notice.read_bytes())
inventory["bundled_components"].append({"name": "rust-standard-library",
                                        "source_version": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                                        "notice_sha256": digest(rust_notice.read_bytes())})
musl = OUTPUT / "musl-COPYRIGHT.txt"
if not musl.is_file():
    raise SystemExit("Supply musl's upstream COPYRIGHT as ui/public/licenses/musl-COPYRIGHT.txt.")
sections.append("\n" + "=" * 72 + "\nmusl C runtime (Linux packages)\n\n" + musl.read_text())
inventory["bundled_components"].append({"name": "musl",
                                        "source": "https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT",
                                        "notice_sha256": digest(musl.read_bytes())})
(OUTPUT / "THIRD-PARTY-NOTICES.txt").write_text("\n".join(sections).replace("\r\n", "\n") + "\n")
for name in ("THIRD-PARTY-NOTICES.txt", "rust-standard-library.html", "musl-COPYRIGHT.txt"):
    inventory["outputs"][name] = digest((OUTPUT / name).read_bytes())
for key in ("rust_packages", "npm_packages", "bundled_components"):
    inventory[key].sort(key=lambda entry: (entry["name"], entry.get("version", "")))
(OUTPUT / "third-party-inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
print(f"Generated notices for {len(inventory['rust_packages'])} Rust and "
      f"{len(inventory['npm_packages'])} production npm packages and bundled native components.")
