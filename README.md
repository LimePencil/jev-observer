# Jev Observer

See what Jev chose, and what changed when you edited a question.

[Website & product tour](https://jev-observer-web.vercel.app/) · [Installation](docs/installation.md) · [Documentation](#documentation-and-project-history)

Jev Observer is a local proxy and dashboard for Jev and compatible System One models, including local Laya servers. It groups recurring questions, keeps their definition versions separate, and shows requests, answers, failures, latency, token usage and reported or estimated cost in one local history.

Built with Rust, SQLite and React, it runs as one executable with the dashboard and fonts included. It is [MIT-licensed](LICENSE), requires no Observer account or subscription, and sends no analytics or automatic event uploads. Live inference goes to your configured provider and remains subject to its charges. This is an independent project; the name is provisional.

![Jev Observer dashboard showing request activity, recurring question groups, usage estimates and collection health](docs/images/overview.png)

The actual web dashboard running in `--demo` mode with 720 synthetic requests. Displayed latency, token usage and costs are sample data.

<details>
<summary>See request details and answer probabilities</summary>

![Jev Observer request details showing a synthetic support-routing decision, answer probabilities and review controls](docs/images/request-details.png)

Inspect individual answers, their reported probabilities and request-level usage, then add a local review label. This example uses synthetic data; no provider was called.

</details>

## What you can do

- Inspect Choice, Score and Noul answers, probabilities and question definitions.
- Track recurring question groups and compare their saved definition versions.
- Browse paginated history, search question groups, share date/source/model filters, and label outcomes correct, incorrect or unknown.
- Review request-level usage, OpenRouter-reported USD cost and configured estimates without counting one request again for each answer.
- Import Observer JSONL or supported JevRouter receipts, and export filtered history as JSONL or CSV.
- Explore 720 synthetic requests in an offline demo with no provider credentials.

## Release and platform status

**0.2.0 is being prepared from source** with local-model support and the [release fixes](docs/releases/0.2.0.md). The published download remains 0.1.0 until the release is tagged.

**[Download 0.1.0](https://github.com/LimePencil/jev-observer/releases/tag/v0.1.0)** for Linux, macOS or Windows. Each release includes six native packages and `SHA256SUMS`.

| Platform | Architectures | Package |
|---|---|---|
| Linux | x86-64 / ARM64 | Static musl executable, `.tar.gz` |
| macOS | Intel / Apple silicon | Native executable, `.tar.gz` |
| Windows | x86-64 / ARM64 | Native MSVC executable, `.zip` |

macOS and Windows executables are unsigned; see [installation](docs/installation.md) for OS policy considerations. WSL2 uses the Linux package and has not been separately tested.

## Install and try the sample

The following commands use anonymous downloads. For private forks, use the [authenticated installation commands](docs/installation.md#private-repository-access).

Linux / macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.sh -o install.sh
sh install.sh --version 0.1.0
export PATH="$HOME/.local/bin:$PATH"
jev-observer --demo
```

Windows PowerShell:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.1.0
& "$env:LOCALAPPDATA\JevObserver\bin\jev-observer.exe" --demo
```

The demo disables upstream forwarding and uses a separate `*.demo.sqlite` database.

Open [http://127.0.0.1:8765](http://127.0.0.1:8765), then sign in with:

- **Username:** `observer`
- **Password:** the token in `.jev-observer/observer.demo.access-token`

Observer prints the exact token-file path at startup, including when using a custom database path. The demo, fonts and charts work offline. Press Ctrl-C to stop it.

Prebuilt releases need no Rust, Node.js or administrator access. See [installation and troubleshooting](docs/installation.md) for private access, anonymous installation after publication, version selection and uninstalling.

## Build from source

Install a recent stable Rust toolchain, a native C compiler, and Node.js 22.12 or later with npm. Windows builds use MSVC and require Visual Studio C++ Build Tools, native Windows Perl (such as Strawberry Perl) and NASM for bundled OpenSSL. Build the dashboard **before** Rust so its assets are embedded:

```sh
npm ci --prefix ui
npm run build --prefix ui
cargo build --release --locked
./target/release/jev-observer --demo
```

In PowerShell, run `npm` and `cargo` as above, then `.\target\release\jev-observer.exe --demo`. If Git Bash Perl is on `PATH`, set `$env:OPENSSL_SRC_PERL` to your native Perl executable before building. SQLite is compiled into the executable. Node.js is needed for development and builds, not to run the finished application.

## Connect an application

Live history requires a saved 32-byte database key. Generate it once and save the printed 64-character value in a password manager. Losing this key makes encrypted history unreadable. Supply the same key at every live startup.

In Bash, read the saved value without putting it in a command-line argument or shell history:

```bash
openssl rand -hex 32
read -r -s -p 'Saved database key: ' JEV_OBSERVER_DB_KEY; echo
export JEV_OBSERVER_DB_KEY
./target/release/jev-observer
```

In PowerShell, generate a key once, save the printed value, then read it without echoing:

```powershell
$keyBytes = New-Object byte[] 32
$random = [Security.Cryptography.RandomNumberGenerator]::Create()
$random.GetBytes($keyBytes)
$random.Dispose()
[BitConverter]::ToString($keyBytes).Replace('-', '').ToLowerInvariant()
$savedKey = Read-Host 'Saved database key' -AsSecureString
$env:JEV_OBSERVER_DB_KEY = [Net.NetworkCredential]::new('', $savedKey).Password
& "$env:LOCALAPPDATA\JevObserver\bin\jev-observer.exe"
```

Run Observer from your application's directory, or choose a stable history location with `--db /path/to/observer.sqlite`. An installed release uses `jev-observer` in place of the source-build path.

Open the dashboard with username `observer` and the token from `.jev-observer/observer.access-token`. In **Connect an application**, enter your provider key and choose session-only storage or your operating system's credential store. Copy the local client token shown once and set it as `JEV_OBSERVER_CLIENT_TOKEN` in your application's environment. If the credential store is unavailable or locked, session-only storage remains available.

Point the SDK at the local **origin**; the tested clients append `/v1/systemone` themselves.

### Python

Tested with `typesafe-sdk==0.7.1`:

```python
import os
from typesafe_sdk import TypeSafeClient

client = TypeSafeClient(
    api_key=os.environ["JEV_OBSERVER_CLIENT_TOKEN"],
    base_url="http://127.0.0.1:8765",
    headers={
        "x-observer-source": "my-application",
        "Accept-Encoding": "identity",
    },
)
```

### JavaScript

Tested with `@typesafe-ai/sdk@0.6.0`:

```javascript
import { TypeSafeClient } from "@typesafe-ai/sdk";

const client = new TypeSafeClient({
  apiKey: process.env.JEV_OBSERVER_CLIENT_TOKEN,
  baseURL: "http://127.0.0.1:8765",
  defaultHeaders: {
    "x-observer-source": "my-application",
    "Accept-Encoding": "identity",
  },
});
```

Observer validates the local client token and substitutes the registered provider key before forwarding. These examples need no additional dashboard-access header. Direct provider-key configurations require `x-observer-access`; see the [connection guide](docs/connection.md) for that setup, credential handling, upstream settings and optional metadata headers.

Both SDKs passed [local mock compatibility checks](docs/compatibility.md#sdk-mock-compatibility). Separate real-inference checks cover [OpenRouter](docs/compatibility.md#live-openrouter-check) and [local Laya](docs/compatibility.md#live-laya-check). The 0.2.0 validator preserves structurally valid Scores and flags cross-field discrepancies as warnings. Identity encoding allows typed-answer capture; compressed bodies pass through but their saved captures are marked incomplete.

### Local Laya (0.2.0)

With Laya already serving on loopback and your saved database key set:

```sh
jev-observer --upstream http://127.0.0.1:8000/v1/systemone \
  --upstream-auth none --provider laya
```

Use the workspace access token as the SDK key and `english` as the request model. Observer authenticates the application locally and sends no authorization to Laya. See [local-model setup](docs/connection.md#laya-and-other-local-system-one-models) for installation, key-protected servers and compatibility limits.

## Inspect, import and export

Open a request to inspect its answers and add a review label. Open a question group to review its distribution and definition versions. Pausing the dashboard holds the visible view while collection continues.

Import up to 10,000 records and 8 MiB of Observer JSONL or supported JevRouter receipts. Export the current filtered history as JSONL or CSV, or delete history from Settings with explicit confirmation. Imported application actions retain their provenance and do not create extra inference charges.

Unknown usage, cost, confidence and outcomes remain unknown. OpenRouter-reported USD cost takes precedence and retains its basis. For cost estimates, supply both `--input-price-per-million` and `--output-price-per-million` in USD. These are user-configured estimates, not invoices or an automatically maintained price list.

## Storage and operating limits

Observer listens on `127.0.0.1:8765` and stores history in `.jev-observer/observer.sqlite` by default. The defaults retain seven days, apply a soft cap of one million records during periodic maintenance, and leave raw input-state persistence off.

Live history is encrypted with SQLCipher. Known credentials are redacted, but retained definitions and answers may still contain sensitive content. Demo data is synthetic and plaintext; JSONL and CSV exports are plaintext too. Stop older Observer processes before migrating an existing plaintext database.

Capture and queue limits can cause gaps in saved history, which collection health reports. Abrupt termination can lose observations, and requests cannot pass through a stopped Observer. See [storage, privacy and operating limits](docs/storage.md) for all defaults, redaction, migration, backups, capacity and shutdown behavior.

## Development and verification

Build the UI and install its dependencies first, then run:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-install.py
npx --prefix ui playwright install chromium
npm test --prefix ui
```

For frontend development, run the backend on port 8765 and `npm run dev --prefix ui`. See the [UI development notes](ui/README.md).

The [verification workflow](.github/workflows/ci.yml) builds the executable, runs Rust and browser checks, and exercises both pinned SDKs against a loopback mock. Browser checks include synthetic API fixtures and a separate journey against the executable and its real authentication, captures, labels, exports and restart. The [release workflow](.github/workflows/release.yml) requires all six native targets to pass Rust, browser, SDK and packaged live/demo checks before publishing; Windows also runs PowerShell installation and upgrade tests.

Native forwarding currently covers `POST /v1/systemone`. Offline threshold previews, matched-dataset replay and universal provider routing are outside the implemented scope. [Performance measurements](docs/performance.md) and the [stress harness](docs/stress.md) include 0.2.0 encrypted-history checks, retained-history query comparisons and overload recovery. Their recorded workloads and shared-host timings do not establish a general capacity limit.

## Documentation and project history

- [Installation, upgrades and troubleshooting](docs/installation.md)
- [Application connection and credential handling](docs/connection.md)
- [SDK compatibility and reproduction](docs/compatibility.md)
- [Storage, privacy and operating limits](docs/storage.md)
- [Implementation contract](docs/implementation-contract.md)
- [Research and launch plan](research/launch.md), [project survey](research/survey/README.md) and [source register](research/sources.md)
- [Historical project guide](reports/README.md) and [validation evidence](reports/validation/README.md)

Research and planning documents describe project history and proposed work; they are not a list of shipped features.

## License

Observer's own code is [MIT-licensed](LICENSE). Every release embeds that notice and the dependency notices at `/licenses/jev-observer-MIT.txt` and `/licenses/THIRD-PARTY-NOTICES.txt`; fonts and bundled components retain their respective licenses. See [license maintenance](docs/licensing.md) for the inventory and regeneration commands.
