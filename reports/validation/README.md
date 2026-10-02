# Validation evidence and reproduction

[Deep audit](deep-audit/README.md) records the subsequent filter, import, startup, health, and export work, with fresh performance and transaction-mutation evidence. Historical evidence below remains unchanged. The retained storage scripts now support the shared parent-reader helper introduced by the CSV optimization.

[Follow-up validation report](followup-bug-validation.md) summarizes the September 22, 2026 prelaunch observations. The JSON results and historical mutation logs contain synthetic local evidence. Public copies replace the original absolute checkout prefix with `<repo>`; recorded observations are preserved. [evidence-manifest.json](evidence-manifest.json) retains the original SHA-256 hashes and byte counts alongside the hashes and sizes of normalized copies. Temporary database paths, source-snapshot filenames and binary paths describe the original run; they are not links to files included in this archive. Earlier development commits are excluded from the fresh release history.

The retained scripts preserve the original probe scenarios, but use ignored `.jev-observer/independent-validation-rerun/` directories for fresh outputs. The source harness pins its baseline revision instead of following `HEAD`, and executable/dependency locations can be configured as described below. Packaging did not rerun the historical application comparisons. Fresh results must be identified by their newly recorded executable/source hashes, rather than attributed to the historical run.

## Inspect evidence without rebuilding

From the repository root, with Python 3:

```sh
python3 reports/validation/verify-evidence.py
```

This verifies archived file hashes and the observations behind the report, including the 14 browser scenarios. It starts no servers and needs no ignored artifacts. Historical browser hashes in [ui/source-manifest.json](ui/source-manifest.json) identify files relative to the original isolated `before/` and `after/` UI snapshots. Those snapshots, including the screenshot entries under `after/artifacts/`, are not bundled. The before source is identified by the prelaunch revision recorded in that manifest; it is not included in the fresh Git history. The after hashes identify the working tree at the time, before subsequent edits.

## HTTP probes

Requires Python 3 and two executable Linux builds. By default the scripts expect `.jev-observer/before-refresh-observer` and `target/release/jev-observer`. Override with absolute `JEV_VALIDATION_BASELINE` and `JEV_VALIDATION_CURRENT` paths. Each probe starts local processes on temporary ports, creates disposable databases, and uses synthetic data; backend forwarding targets its own loopback mock.

The historical baseline binary is intentionally not committed. Its source provenance was not independently reconstructed, so rebuilding archived baseline source is not claimed to reproduce that binary. An exact rerun of the recorded binary comparison requires the original executables with the report's hashes. A fresh `cargo build --release --locked` supplies a new current executable; record its actual hash for new comparisons.

```sh
python3 reports/validation/backend/reproduce_backend.py
python3 reports/validation/storage/check_groups.py
python3 reports/validation/storage/check_http_races.py
```

These scripts collect observations; they do not automatically fail on every product regression. Review the fresh JSON results separately. The HTTP race is scheduler-dependent, so its failure counts need not match the historical counts.

## Deterministic storage harness

Requires Rust, compiled dependencies, and archived baseline source. No historical binary is needed. Set `JEV_VALIDATION_BASE_SOURCE` to an archived `store.rs`; fresh clones exclude the original baseline commit. Alternatively, a development archive containing that commit can use `JEV_VALIDATION_BASE_REV`. The harness copies baseline/current source, inserts a hook after the parent read, and compiles a separate executable outside the product tree. A separate writer commits deletion or replacement before the public read resumes. Generated sources and binaries stay in the ignored scratch directory.

The default dependency directory is `target/debug/deps`. It must contain exactly one `.rlib` for each of `anyhow`, `chrono`, `rusqlite`, `serde`, and `serde_json`, all built by the active Rust toolchain. If an existing target directory has multiple variants, create a fresh dependency build and point the harness to it:

```sh
cargo build --locked --target-dir .jev-observer/independent-validation-rerun/snapshot-deps
JEV_VALIDATION_BASE_SOURCE=/path/to/archived/store.rs JEV_VALIDATION_DEPS="$PWD/.jev-observer/independent-validation-rerun/snapshot-deps/debug/deps" python3 reports/validation/storage/check_snapshots.py
```

`JEV_VALIDATION_BASE_SOURCE` takes precedence over `JEV_VALIDATION_BASE_REV`. The harness requires its parent-read insertion marker to occur exactly once and fails if the source structure has changed. The resulting manifest records the chosen source provenance and both source hashes; the historical `head`/`working` labels in result fields mean baseline/current inputs to that run.

## Public regression mutation audit

The [later audit](test-quality/public-snapshot-regression/results.json) checks the five new public-method snapshot tests against isolated copies with the request wrapper, export wrapper, or both removed. It uses the same dependency setup as the storage harness, plus the `sha2` and `tempfile` test dependencies. If creating a fresh dependency directory, use `cargo test --locked --no-run --target-dir .jev-observer/independent-validation-rerun/snapshot-deps` so those test dependencies are built. A release executable must exist at `target/release/jev-observer` (or `JEV_VALIDATION_CURRENT`); the audit only hashes it to verify it remains unchanged, and does not execute it.

```sh
JEV_VALIDATION_DEPS="$PWD/.jev-observer/independent-validation-rerun/snapshot-deps/debug/deps" python3 reports/validation/test-quality/public-snapshot-regression/check_mutations.py
```

The audit fails unless all five tests pass against current source, two request tests fail when only the request wrapper is removed, three export tests fail when only the export wrapper is removed, and all five fail when both are removed. Its source-string markers intentionally fail if later source changes require an updated mutation. The result manifest records the actual source hashes, commands, and fresh run date.

## Browser scenarios

Requires Node.js, `npm ci` in `ui/`, Playwright Chromium (`cd ui && npx playwright install chromium`), a current release backend, and production UI builds at `.jev-observer/independent-validation-rerun/ui/{before,after}/dist`. Loopback ports 19861–19863 must be free. The historical before UI must be supplied from a development source archive; it cannot be reconstructed from the fresh Git history. To prepare comparison snapshots from the repository root:

```sh
mkdir -p .jev-observer/independent-validation-rerun/ui/before
cp -R /path/to/archived/ui/. .jev-observer/independent-validation-rerun/ui/before/
python3 - <<'PY'
from pathlib import Path
from shutil import copytree, ignore_patterns
destination = Path('.jev-observer/independent-validation-rerun/ui/after')
copytree('ui', destination, ignore=ignore_patterns('node_modules', 'dist', 'artifacts', 'test-results', 'playwright-report'))
PY
npm --prefix .jev-observer/independent-validation-rerun/ui/before ci
npm --prefix .jev-observer/independent-validation-rerun/ui/before run build
npm --prefix .jev-observer/independent-validation-rerun/ui/after ci
npm --prefix .jev-observer/independent-validation-rerun/ui/after run build
node reports/validation/ui/reproduce.mjs
```

Use a fresh scratch directory for each comparison; the copy step refuses to replace an existing after snapshot. `JEV_VALIDATION_UI_DIR` selects another directory containing the two builds, and `JEV_VALIDATION_CURRENT` selects the backend executable. The script uses a private database there and routes both UI snapshots to that real backend. It records new JSON, screenshots, and accessibility trees in the scratch directory. Preserve the chosen source hashes alongside any fresh results. Browser version and native clipboard behavior can differ from the historical Chromium 153 run.
