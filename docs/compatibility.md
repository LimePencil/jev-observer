# Compatibility checks

## Live OpenRouter check

Checked October 2, 2026 against `https://openrouter.ai/api/v1/systemone`, using a Linux ARM64 0.2.0 candidate and synthetic ticket data. **All 51 checks passed.** The requested model was `jev-1.13`; OpenRouter returned `typesafe/jev-1.13-20260917`. This check used Python's standard HTTP client; it does not establish live compatibility of either official SDK or the direct TypeSafe service.

Four real inference requests returned HTTP 200 with **12 valid Choice, Noul and Score answers**. An invalid model returned 400; a wrong local token returned 401 without forwarding. All five upstream attempts were saved completely, with no dropped captures or write failures. Direct provider-key authentication and registered session credentials both worked. Definition changes produced four groups; labels, JSONL/CSV exports, duplicate reimport and encrypted restart passed. Exports contained neither test credentials nor the synthetic input-state marker.

One Score differs from the weighted displayed probabilities. The candidate preserves both original fields and shows a consistency warning; structural invalidity still excludes an answer from valid statistics. The warning uses a `0.001` difference threshold. This is an explicit presentation policy, not a claim that the provider's rounding behavior is understood. The original [0.1.0 result](../reports/validation/next-release/openrouter-live.json) remains unchanged and failed two assertions because it rejected that answer. The observed values are covered by a deterministic model regression test.

Usage matched exactly at **1,639 input and 284 output tokens**, and OpenRouter-reported cost matched **$0.000068838** with `provider_reported` provenance. These amounts are a small functional test, not invoice reconciliation or performance evidence. Custom providers' undocumented `usage.cost` values are retained as extensions and are not assumed to be USD.

The [candidate live result](../reports/validation/next-release/openrouter-0.2.0.json) records the executable hash, all checks, typed answers, warnings and usage. The [release assessment](releases/next-release-readiness.md) tracks the related reliability fixes and verification limits.

To repeat this opt-in test, supply a key through an existing environment variable or a literal dotenv assignment. It makes four inference calls and one invalid-model call, uses a temporary encrypted workspace, and removes that workspace afterward:

```sh
python3 scripts/live-openrouter-smoke.py \
  --binary /path/to/jev-observer \
  --env-file /path/to/private.env \
  --key-name OPENROUTER_API_KEY \
  --output /tmp/observer-openrouter-result.json
```

Omit `--env-file` to use `OPENROUTER_API_KEY` from the environment. The script never executes the dotenv file or prints its key. See [OpenRouter connection setup](connection.md#jev-through-openrouter).

## Live Laya check

Checked October 2, 2026 with **real local CPU inference**, `laya[serve]==0.3.23`, and the English checkpoint from `convaiinnovations/laya` at revision `55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851`. All **16 checks passed** through the 0.2.0 candidate. Three requests produced nine valid Choice, Noul and Score answers and four definition groups, with 441 input tokens and zero output tokens. No cost was invented. Access rejection, encrypted storage, state exclusion, provider attribution and gap counters passed.

The Laya adapter accepts four-decimal distributions using a sum tolerance of `max(0.0001, 0.00005 × option_count)` plus floating-point epsilon, supports string-list Choice criteria and one to 32 Score levels, and preserves extension fields. Range, keys, type and confidence checks still apply. The reviewed [upstream HTTP implementation](https://github.com/NandhaKishorM/laya/blob/4aa6761be8173de4ce6d92c31b3e40b6eaf59a7c/laya/serve.py) accepts the System One endpoint. Batch routes, extended numeric Choice labels, abstention-specific semantics, other checkpoints and model accuracy/calibration are outside this claim.

The [sanitized Laya result](../reports/validation/next-release/laya-live.json) records model revision, installed server hash, executable hash, responses and checks. The timings include cold inference and shared-host compilation; they are not a throughput benchmark. Follow [local-model setup](connection.md#laya-and-other-local-system-one-models), then reproduce against an already-running model server:

```sh
python3 scripts/local-systemone-smoke.py \
  --binary target/release/jev-observer \
  --upstream http://127.0.0.1:8000/v1/systemone \
  --provider laya --model english \
  --output /tmp/observer-laya-result.json
```

## SDK mock compatibility

Rechecked October 2, 2026 against a local mock upstream with the 0.2.0 candidate. **Both official SDKs passed with direct provider credentials and registered local client tokens.** No paid inference, provider connection or real API credential was used. This verifies the listed SDK versions and fixtures; it is not a live TypeSafe compatibility guarantee or a performance benchmark.

| Client | Package tested | Runtime tested | Proxy setting |
|---|---|---|---|
| Python synchronous client | `typesafe-sdk==0.7.1` | Python 3.12.13 | `base_url="http://127.0.0.1:8765"` |
| JavaScript client | `@typesafe-ai/sdk@0.6.0` | Node.js 24.15.0 | `baseURL: "http://127.0.0.1:8765"` |

Use the origin as the SDK base URL, without `/v1/systemone`: both tested clients append that route. The Python client accepts additional headers through `headers` / `extra_headers`; JavaScript accepts `defaultHeaders` / per-call `headers`. Set `x-observer-source` to distinguish applications; Observer removes its local metadata headers before forwarding. These options are documented in the official [Python client reference](https://docs.typesafe.ai/sdk/python/api/clients/sync) and [JavaScript client configuration](https://docs.typesafe.ai/sdk/javascript/api/interfaces/TypeSafeClientConfig).

For a key registered through Observer's **Connect an application** panel, pass the one-time local client token as the SDK's `api_key` (Python) or `apiKey` (JavaScript). Observer substitutes the registered provider key before the upstream request; a wrong or rotated local token is rejected locally. SDK configurations that pass their provider key directly also need `x-observer-access` with the workspace access token. The registered local token needs no additional access header. The token is not recoverable from the status API, so copy it when shown.

For typed-answer capture, request uncompressed responses with `headers={"Accept-Encoding": "identity"}` in Python or `defaultHeaders: { "Accept-Encoding": "identity" }` in JavaScript. Both tested configurations include this header, and the mock verifies it reaches the upstream on every request. Observer forwards compressed bodies unchanged but currently marks their saved captures incomplete instead of decoding them. See the [complete client setup examples](../README.md#connect-an-application). An upstream that still returns compressed data cannot provide complete typed capture in this version.

## Verified behavior

Each SDK sent two repeated requests containing Choice, Score and Noul questions, one request with changed Choice instructions, and one request receiving a mock HTTP 422. The same four-call sequence ran first with a direct provider credential and then with a registered local client token. Python used the SDK's typed question constructors for the successful calls; JavaScript used `systemOne` with the native question objects.

- Both SDKs received the mock's success and error response bytes unchanged, including unknown fields and a custom response header. The expected SDK error exposed status 422 and the original error body.
- Unknown request fields reached the mock. Observer retained unknown response, answer and usage fields in its normalized extension fields.
- Both SDKs forwarded `Accept-Encoding: identity`; all 16 saved captures were complete.
- Sixteen attempts produced 16 parent request records and 48 answer observations; 36 answers were valid. Failed-request placeholders were excluded from distributions.
- Repeated definitions accumulated together. Changed Choice instructions created a separate group; Noul and Score groups continued. The two source namespaces produced eight groups total.
- Usage totaled 1,200 input and 144 output tokens, counted once per successful parent request. Twelve requests had a configured estimate; the four errors had unknown cost. The test's explicit artificial rates produced $0.002544. This is fixture arithmetic, not TypeSafe pricing.
- Local source headers and local client tokens did not reach the upstream. A dummy provider credential echoed in the response was redacted in saved data; the registered token was absent from saved records. Raw input state was not retained. No captures were dropped.

The [candidate result](../reports/validation/next-release/sdk-0.2.0.json) includes the binary SHA-256, runtimes, counters and check time. Its latency values come from 16 requests to a Python fixture server and are not performance evidence. Python's raw transport response and Observer's retained extensions are checked independently of how the SDK exposes additional fields through its typed response model.

## Reproduce

Install dependencies into an isolated temporary directory. These installation steps access package registries; the test itself guards both SDK transports to the exact loopback proxy origin and configures Observer to forward only to its own loopback mock.

```bash
cargo build --release
SDK_WORK=$(mktemp -d)
uv venv --python 3.12 "$SDK_WORK/python"
uv pip install --python "$SDK_WORK/python/bin/python" -r fixtures/sdk/python-requirements.txt
cp fixtures/sdk/package.json fixtures/sdk/package-lock.json "$SDK_WORK/"
npm ci --prefix "$SDK_WORK" --ignore-scripts --no-audit --no-fund
"$SDK_WORK/python/bin/python" scripts/sdk-compatibility.py \
  --binary target/release/jev-observer \
  --node-sdk "$SDK_WORK/node_modules/@typesafe-ai/sdk" \
  --output "$SDK_WORK/result.json"
```

The runner starts its own proxy process and mock server on temporary loopback ports, uses a temporary database, and shuts them down afterwards. SDK retries are explicitly disabled so the upstream-attempt count is exact. Normal applications can retain their SDK retry policy; Observer does not add another retry layer.

The fixtures and package pins are under [fixtures/sdk](../fixtures/sdk). No streaming behavior, asynchronous Python client, model-list endpoint, browser SDK, provider gateway dialect or actual upstream service was exercised by this check. Other endpoints remain outside the native `POST /v1/systemone` forwarding scope.
