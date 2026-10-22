# Installation

Jev Observer 0.4.0 runs as one executable with its dashboard, fonts and SQLite included. Installation needs no Rust, Node.js, database server or administrator access.

## Install a release

These commands download the public release without authentication. If you are accessing a private copy, use [private repository access](#private-repository-access) below.

### Linux and macOS

```sh
curl -fsSL https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.sh -o install.sh
sh install.sh --version 0.4.0
export PATH="$HOME/.local/bin:$PATH"
jev-observer --demo
```

### Windows PowerShell

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.4.0
$env:PATH = "$env:LOCALAPPDATA\JevObserver\bin;$env:PATH"
jev-observer --demo
```

Inspect the downloaded script before running it if desired. If your Windows execution policy blocks scripts, follow your organization's policy or download and extract the matching release ZIP manually; run its `jev-observer.exe` directly.

Open <http://127.0.0.1:8765> and sign in with username `observer` and the token in `.jev-observer/observer.demo.sqlite.token`. Observer prints the exact token-file path at startup. Demo mode uses 720 synthetic requests, makes no provider calls and needs no database key. Press Ctrl-C to stop it.

For live requests, [generate and save a database key](../README.md#connect-an-application), supply it on each startup, then run without `--demo` and connect your application's SDK. The live dashboard token is in `.jev-observer/observer.sqlite.token`.

Version 0.4.0 uses the `.sqlite.token` paths shown here. Upgrading from 0.3.1 or earlier rotates the workspace dashboard token once; see [history backups and downgrades](#history-backups-and-downgrades). The exact path printed at startup is authoritative for the executable you run.

**Prelaunch builds:** discard development binaries from the earlier private release history and install the current public release before collecting live traffic. Release numbers from that discarded history do not identify the current build.

### Private repository access

Install [GitHub CLI](https://cli.github.com/) and sign in with `gh auth login` using an account that can read `LimePencil/jev-observer`.

Linux / macOS:

```sh
gh api --hostname github.com repos/LimePencil/jev-observer/contents/install.sh \
  -H 'Accept: application/vnd.github.raw+json' > install.sh
sh install.sh --version 0.4.0
```

Windows PowerShell:

```powershell
gh api --hostname github.com repos/LimePencil/jev-observer/contents/install.ps1 `
  -H 'Accept: application/vnd.github.raw+json' | Set-Content -Encoding UTF8 install.ps1
.\install.ps1 -Version 0.4.0
```

Anonymous downloads require both the repository and release to be public. Private asset downloads use existing GitHub CLI authentication without printing or saving your GitHub token.

## Versions, upgrades and paths

The installers detect the native OS and CPU, verify the archive's SHA-256 checksum and executable version, then install into `~/.local/bin` on Unix or `%LOCALAPPDATA%\JevObserver\bin` on Windows. They do not edit shell profiles, request administrator access, start background services or configure provider credentials.

Omit the version option to install the latest published release. Rerun to upgrade; failed downloads, checks or replacements preserve the existing executable. **Stop Observer before upgrading**, especially on Windows where a running executable may be locked. Upgrading does not itself move, delete or migrate history.

Choose a different installation directory:

```sh
sh install.sh --version v0.4.0 --install-dir "$HOME/bin"
```

```powershell
.\install.ps1 -Version v0.4.0 -InstallDir "$env:USERPROFILE\bin"
```

The installer prints a command to add that directory to the current terminal's `PATH`. To keep it available in new terminals, add the Unix export to your shell configuration or add the Windows directory to your user's `Path` environment variable.

Check the executable with `command -v jev-observer` on Unix or `Get-Command jev-observer` in PowerShell, then run `jev-observer --version`.

### History backups and downgrades

Before upgrading to 0.4.0, stop Observer cleanly and keep a copy of the database and any remaining SQLite sidecars, both current and legacy workspace dashboard-token files, and access to the saved `JEV_OBSERVER_DB_KEY`. Preserve the earlier executable if you need to test a rollback. Starting the new executable upgrades supported query indexes; a rejected unsupported database is not encrypted or otherwise rewritten. Copy files back only while every Observer process using that workspace is stopped.

**New answer kinds limit rollback.** Valid Box observations store geometry without a numeric value. Earlier readers assume valid non-Choice answers are numeric, so their dashboard or group queries can fail on a workspace containing Box records even though the database schema has not changed. A binary-only downgrade is not a supported way to read that history. Older versions also do not understand the native Decisions protocol, typed Choice identities or new protocol/profile flags.

For rollback, retain the upgraded workspace separately and restore the stopped pre-upgrade snapshot for the older executable. That snapshot excludes observations collected after the upgrade; keep the 0.4.0 workspace so they remain available when returning to 0.4.0. Exporting new Box or native Decisions records as JSONL does not make them compatible with older normalizers. Preserve the previous startup and client configuration as well as the executable.

Version 0.4.0 uses the complete database filename followed by `.token`, such as `observer.sqlite.token` or `observer.demo.sqlite.token`; published 0.3.1 uses the legacy `*.access-token` path. This keeps custom databases such as `team.db` and `team.sqlite` from sharing the old `team.access-token` path. An existing new-format token is reused with the usual private-file checks. When only a legacy `*.access-token` file exists, the first startup creates a fresh random token and prints a notice; it never copies the legacy secret. Later startups reuse the new token.

After that one-time rotation, sign in again as `observer` with the new token and update clients that use the workspace token in Basic auth, `X-Observer-Access`, or `--upstream-auth none` requests. This includes direct provider-key and configured-key fallback clients that supply workspace authentication. Database keys, history, saved provider credentials and registered `jo_local_` client tokens are unchanged by the token-path upgrade; session-only credentials still expire on restart as usual.

Legacy token files remain untouched for rollback. An older executable will use its legacy token again and retain the old same-stem sharing behavior; the new token does not protect a downgraded installation. Keep both files private, and use separate database directories if you must run an older executable with same-stem custom database names. Do not copy a shared legacy token into the new token files.

Version 0.2.0 reads provider credentials saved by 0.1.0. After saving or rotating a persisted provider key in 0.2.0, the new credential entry layout is not readable by 0.1.0. If you downgrade, re-register the provider key and update the application's local client token. Restoring only an older SQLite backup cannot restore an operating-system credential entry removed by a later rotation. Session-only credentials always require registration after restart. The database key and provider key are separate secrets.

On Unix, builds containing the native credential-path update identify a workspace using the exact bytes of its canonical database path. Valid UTF-8 paths and canonical aliases keep their existing credential accounts. If any component of the canonical path contains invalid UTF-8 bytes, re-register the persisted provider key and update the application's local client token after upgrading. Earlier builds replaced invalid bytes when naming credential accounts, so distinct paths could share an account. The new account does not read, copy or remove that ambiguous old account; old entries are left untouched. An older executable still uses the old naming behavior and cannot find credentials registered under the new account. New registration changes SQLite’s approval digest, so a downgrade does not automatically reactivate an old entry: re-register the provider key or restore an appropriate matching backup, as described above. This change does not alter history, database keys or dashboard access tokens; non-Unix account naming is unchanged.

The automated upgrade checks exercise encrypted history, labels, import identities, approval digests, backups and rollback using disposable legacy-record fixtures. They do not establish that new answer kinds are readable by older releases. Native Linux credential-store compatibility is checked separately; this does not establish every operating-system keychain or older-version combination. Reconnect the application and verify collection health before resuming normal traffic after a rollback.

## Supported packages

| Platform | Architecture | Rust target | Archive |
|---|---|---|---|
| Linux | x86-64 / AMD64 | `x86_64-unknown-linux-musl` | `.tar.gz` |
| Linux | ARM64 | `aarch64-unknown-linux-musl` | `.tar.gz` |
| macOS | Intel | `x86_64-apple-darwin` | `.tar.gz` |
| macOS | Apple silicon | `aarch64-apple-darwin` | `.tar.gz` |
| Windows | x86-64 | `x86_64-pc-windows-msvc` | `.zip` |
| Windows | ARM64 | `aarch64-pc-windows-msvc` | `.zip` |

All six packages must pass native release verification before publication. Tests run on Ubuntu 24.04, macOS 15, Windows Server 2025 x86-64 and Windows 11 ARM64; this does not establish compatibility with every older OS version. WSL2 uses the Linux executable and has not been separately tested. Other architectures have no prebuilt package.

Linux packages include the C runtime and SQLite, removing a dependency on the distribution's glibc version. Windows packages statically link the C runtime and do not require a separate Visual C++ redistributable. Live forwarding still requires networking and DNS. macOS and Windows packages are unsigned and not notarized; OS security policies may block them or require approval. SHA-256 verifies downloaded-byte consistency, not publisher identity. The installer reports a missing platform package rather than selecting another architecture.

## History and uninstalling

History is relative to the directory where Observer starts, by default `.jev-observer/observer.sqlite`. Demo history uses `.jev-observer/observer.demo.sqlite`. The installation directory does not determine where history is saved.

Use `--db /path/to/observer.sqlite` to choose a stable location. On Windows, the default `.jev-observer` directory receives a private current-user ACL. A custom database directory must already restrict access to the current user, SYSTEM and administrators; shared directories are rejected. Access-token files have a protected current-user ACL, and reparse points are rejected.

The dashboard token for that custom path is `/path/to/observer.sqlite.token`. The full database filename is preserved when adding the suffix, including names without an extension or with multiple dots.

To uninstall, stop Observer and remove the executable:

```sh
rm "$HOME/.local/bin/jev-observer"
```

```powershell
Remove-Item "$env:LOCALAPPDATA\JevObserver\bin\jev-observer.exe"
```

History stays in place. Remove it separately only when you intend to discard those records. Remove the corresponding executable if you chose a custom installation path.

## Troubleshooting

- **GitHub returns 404:** verify repository access with `gh repo view LimePencil/jev-observer` and authenticate with `gh auth login` if needed.
- **Command not found or wrong version:** use the installer's printed `PATH` command and check which executable the shell finds; try its absolute path.
- **Checksum, archive or version mismatch:** installation stops before replacing the executable. Retry and report the version, platform and error if it persists.
- **Windows refuses an upgrade:** stop the running Observer process and try again.
- **Permission denied:** choose a user-writable installation directory. For a Windows custom database directory, also check its ACL as described above.
- **OS blocks an unsigned executable or script:** follow your organization's security policy; manual archive extraction is available when script execution is restricted.
- **Port 8765 is in use:** stop the other process or run `jev-observer --port 8766` and open the matching local address.

## Maintainer release packaging

Build the dashboard before Rust, then package on the matching native platform. For Linux x86-64:

```sh
npm ci --prefix ui
npm run build --prefix ui
rustup target add x86_64-unknown-linux-musl
CC=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl
python3 scripts/package-release.py \
  --binary target/x86_64-unknown-linux-musl/release/jev-observer \
  --target x86_64-unknown-linux-musl --output-dir .jev-observer/release
```

Linux musl builds need a matching musl C compiler. Packaging needs Python 3.11 or later, verifies OS/architecture, executable version, authenticated live and demo startup, all 720 demo requests and every embedded dashboard file, then produces a single-executable archive and `SHA256SUMS`. It refuses an existing archive.

The [release workflow](../.github/workflows/release.yml) runs six native builds and publishes only after every verification job succeeds. A manual dispatch verifies packages without publishing. Version tags must match `Cargo.toml`; published releases cannot be replaced. Publication checks the complete six-archive set and aggregate manifest before making the draft available.

Installer regressions use local fixture servers and temporary directories:

```sh
python3 scripts/test-install.py
```

On Windows, after native packaging:

```powershell
python scripts/test-install-windows.py --assets release-assets
```
