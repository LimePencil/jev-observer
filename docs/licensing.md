# License maintenance

Observer's own source is covered by the root [MIT license](../LICENSE). Dependencies retain their own licenses. Releases contain the executable with all notices embedded in its dashboard assets:

- `/licenses/jev-observer-MIT.txt`: generated from the root license during the UI build.
- `/licenses/THIRD-PARTY-NOTICES.txt`: notices for the locked Rust graph across all six supported targets, production npm dependencies, SQLCipher, OpenSSL and the musl C runtime.
- `/licenses/third-party-inventory.json`: component versions, source provenance and input/output hashes.
- `/licenses/rust-standard-library.html`: the Rust toolchain's standard-library copyright and license inventory.
- `/licenses/geist-OFL.txt` and `/licenses/geist-mono-OFL.txt`: bundled font licenses.

These local endpoints use the same dashboard authentication. The single executable includes the notices without extra installation files.

Builds use Rust 1.98.1, pinned in `rust-toolchain.toml` and CI, so the bundled standard-library notices match the compiler. A compiler upgrade requires regenerating and reviewing those notices.

## Regenerate after changing dependencies

Install [cargo-about 0.9.2](https://github.com/EmbarkStudios/cargo-about/releases/tag/0.9.2), fetch Cargo sources, install production and development UI dependencies, and install Rust documentation:

```sh
cargo fetch --locked
npm ci --prefix ui
rustup component add rust-docs
python3 scripts/generate-license-notices.py
python3 scripts/generate-license-notices.py --check
npm run build --prefix ui
```

`--cargo-about /path/to/cargo-about` selects a standalone binary. The generator uses [about.toml](../about.toml) to cover every native release target and includes build dependencies. It retains each collected notice's copyright text, includes the vendored SQLCipher and OpenSSL notices, and records production npm package versions.

Some npm archives omit their license files. [Pinned supplemental notices](../licenses/npm-supplemental.json) record the upstream source, revision, checksum and any provenance limitation. Generation fails for an unrecorded missing notice or mismatched installed package version.

The musl notice is copied from its [upstream COPYRIGHT](https://git.musl-libc.org/cgit/musl/plain/COPYRIGHT); review it when updating the runtime/toolchain. Review and regenerate the standard-library inventory after changing Rust. Check mode verifies the active compiler version and runs without cargo-about, Rust documentation or a network connection. Source CI and every native packaging job verify the lockfile and notice hashes; packaging also checks the actual embedded bytes against the current source assets.
