# Connect an application

Generate a 32-byte database key and save the printed 64-character value in a password manager. Observer needs the same key on every live startup; losing it makes encrypted history unreadable. Supply it without putting the value in a command-line argument or shell history:

```bash
openssl rand -hex 32
read -r -s -p 'Saved database key: ' JEV_OBSERVER_DB_KEY; echo
export JEV_OBSERVER_DB_KEY
```

Start normal collection:

```bash
jev-observer
```

Open the local dashboard with username `observer` and the token in `.jev-observer/observer.sqlite.token`, whose path Observer prints at startup. This owner-only token protects local history and key settings from other accounts on the same computer. Keep it private. SDK requests using a registered local client token need no additional header; SDK requests using their provider key directly must also send this access token in `x-observer-access`.

The token path appends `.token` to the complete database filename, including its extension: `team.db` and `team.sqlite` use `team.db.token` and `team.sqlite.token`. Existing new-format tokens are reused on restart. If only a legacy `*.access-token` file exists, Observer creates a fresh token and prints an upgrade notice. Update the browser's Basic-auth password and any workspace token used in `X-Observer-Access` or with `--upstream-auth none`. This also affects configured provider-key fallback calls authenticated with Basic auth or `X-Observer-Access`. Registered `jo_local_` client tokens and their provider credentials are not rotated by this change; their existing session/system persistence rules still apply. See [upgrade and rollback guidance](installation.md#history-backups-and-downgrades).

Run it from your application's directory to keep history in that project's `.jev-observer/` folder, or choose a stable location with `--db /path/to/observer.sqlite`. For a source build, use `./target/release/jev-observer` instead.

For System One, point your application's TypeSafe SDK at the local **origin**, without appending `/v1/systemone`. The tested clients append that path themselves. OpenAI Decisions uses the origin plus `/v1`; Cloudflare uses native HTTP, as described below. Keep your existing credentials in the application environment.

Alternatively, open **Connect an application** in the dashboard, enter your provider key, and choose session-only storage or your operating system's credential store. Observer returns a local client token. Put that token in the SDK's `api_key` / `apiKey` setting instead of the provider key. Observer validates the token and substitutes the registered provider key before forwarding. Registration and removal continue if the panel closes; reopen it to see the result. The returned token stays in page memory across panel changes until the page reloads or closes, or the key is successfully replaced or removed. Keep the token private; losing it requires registering the provider key again to rotate the token. An unavailable or locked system credential store returns an error; session-only storage remains available, and a failed replacement keeps the previous token available.

Python (`typesafe-sdk==0.7.1`):

```python
import os
from typesafe_sdk import TypeSafeClient

client = TypeSafeClient(
    api_key=os.environ["TYPESAFE_API_KEY"],
    base_url="http://127.0.0.1:8765",
    headers={"x-observer-source": "my-application", "x-observer-access": os.environ["JEV_OBSERVER_ACCESS_TOKEN"], "Accept-Encoding": "identity"},
)
```

JavaScript (`@typesafe-ai/sdk@0.6.0`):

```javascript
import { TypeSafeClient } from "@typesafe-ai/sdk";

const client = new TypeSafeClient({
  apiKey: process.env.TYPESAFE_API_KEY,
  baseURL: "http://127.0.0.1:8765",
  defaultHeaders: { "x-observer-source": "my-application", "x-observer-access": process.env.JEV_OBSERVER_ACCESS_TOKEN, "Accept-Encoding": "identity" },
});
```

Set `JEV_OBSERVER_ACCESS_TOKEN` to the value in the workspace access-token file when using the examples with a direct provider key. For a key registered in Observer, put the displayed local client token in `JEV_OBSERVER_CLIENT_TOKEN` and use that variable for `api_key` / `apiKey` in either example; the `x-observer-access` header can then be omitted. Do not put the provider key in the application when using this mode.

The examples request identity encoding so typed answers can be inspected. Compressed bodies still pass through unchanged, but this version marks their saved captures incomplete instead of decoding them.

Both SDKs passed the [reproducible local compatibility check](compatibility.md), including success/error forwarding, definition grouping and request-level usage accounting. Those checks use a loopback mock and dummy credentials; they do not establish compatibility with every provider or SDK version.

Native forwarding covers `POST /v1/systemone`; the default destination is `https://api.typesafe.ai/v1/systemone`. Configure a different fixed endpoint with `--upstream URL`. Remote upstreams require HTTPS; HTTP is accepted for loopback servers. Caller authorization takes precedence over an optional `TYPESAFE_API_KEY` fallback in Observer's environment. Direct provider-key and environment-fallback requests must send the dashboard token in `X-Observer-Access`; the fallback also requires an `application/json` Content-Type. Provider keys cannot be passed as command-line arguments. Observer adds no upstream retries, caching or redirects.

Observer allows **300 seconds for the entire upstream exchange**, including request upload, inference and response-body transfer. Change this with `--upstream-timeout-secs SECONDS` (1–86,400); connection establishment is still limited to 15 seconds. There is no shorter read timeout while waiting for inference. A deadline reached before response headers returns HTTP 504; a deadline reached during the response body ends the stream and records an incomplete transfer. The SDK's own timeout also applies.

The deadline continues while a slow client stops reading. Observer drops unfinished upstream work and releases its capture reservation at the deadline, preserving a visible incomplete observation. Reads remain demand-driven without a separate response buffer. A response whose declared body has already been fully observed is not changed into a timeout while waiting for the client; a blocked downstream socket can remain open until the client reads or disconnects.

## Protocol, model profile, and provider

These settings describe different parts of a connection:

- `--protocol systemone|openai-decisions|cloudflare-systemone` selects the request/response contract that Observer captures.
- `--model-profile CATALOG_ID` identifies the reviewed model integration and its normalization rules. It is separate from the request's model name.
- `--provider NAME` identifies the credential and billing provider, which may serve several model families.

The connection panel shows the running settings and generates matching examples. Selecting another catalog entry only prepares a startup command; restart Observer to change its fixed upstream. Replace account placeholders in catalog endpoints with your own account ID. Offline catalog entries provide mapped-capture import instructions and no forwarding endpoint. Observer does not install or start a model server.

### OpenAI Decisions

Start Observer with the native Decisions contract, then register the provider key in the connection panel:

```sh
jev-observer --upstream https://api.openai.com/v1/decisions \
  --provider openai --protocol openai-decisions --model-profile openai-decisions
```

Use the local client token returned by registration. The native client calls `POST /v1/decisions`, with `input` and an ordered `questions` array. Python requires `openai>=3.26.0`; JavaScript requires `openai>=7.30.0` for this API. The connection panel generates examples for both languages. For Python:

```python
import os
from openai import OpenAI

client = OpenAI(
    api_key=os.environ["JEV_OBSERVER_CLIENT_TOKEN"],
    base_url="http://127.0.0.1:8765/v1",
    max_retries=0,
    timeout=60,
    default_headers={"x-observer-source": "my-application", "Accept-Encoding": "identity"},
)
response = client.decisions.create(
    model="gpt-6-luna",
    input="Please help me reset my password.",
    questions=[{"type": "predicate", "name": "needs_help", "instructions": "Does the message ask for help?"}],
)
print(response)
```

The model must support Decisions for your account. Running this example sends an inference request and may incur charges. Observer records each question's ordinal key (such as `question:0`) and preserves its original native definition. Predicates appear as yes-probabilities; native Boolean and String choices retain distinct identities, including `true` versus `"true"`. Probability labels and group bins show that distinction.

### Cloudflare System One

Choose a Cloudflare catalog profile and replace its account placeholder. Keep `--provider cloudflare` and `--protocol cloudflare-systemone`, with the chosen `--model-profile`. Applications send native System One JSON to Observer's local `POST /v1/systemone` route. Cloudflare returns an outer object with `success`, `result`, `errors`, and `messages`; the typed response lives inside `result`.

Use the connection panel's Python standard-library or JavaScript `fetch` example. It checks both HTTP success and the envelope's `success` field before reading `result`. A TypeSafe SDK that expects an unwrapped response cannot consume this envelope directly. Observer preserves the forwarded envelope and normalizes its inner typed response for local inspection.

### Box geometry and provider extensions

Box answers retain four normalized edges: `top`, `bottom`, `left`, and `right`. The request view shows exact coordinate values and a unit-plane outline; it does not fetch the image or claim its aspect ratio. Reported confidence is separate from the geometry. Invalid geometry remains inspectable through the raw-answer disclosure. Box groups expose counts, reviews, and individual observations, without a scalar mean or geometry histogram.

For H2O responses, a reported `found:false` suppresses the fallback rectangle and is shown as no localization. It does not establish target absence. Optional `present` evidence is displayed separately from confidence, including zero values; its interpretation depends on the provider's mode. The raw response retains the original extension fields and fallback coordinates.

Use **Decision request/response capture** to import original calls. Its compatibility format identifier remains `systemone-capture`; add `protocol: "openai-decisions"` for native ordered question/answer arrays or `protocol: "cloudflare-systemone"` for a wrapped response. The optional `model_profile` selects model-specific normalization. Omitted protocol keeps the existing System One contract.

JSONL exports preserve typed answers, original definitions, protocol, model profile, and raw answer provenance. CSV remains the existing request-summary format: one row per request with request usage, cost, and answer count. It does not add geometry coordinates or typed-choice columns.

## Jev through OpenRouter

After setting your saved `JEV_OBSERVER_DB_KEY`, start Observer with OpenRouter's fixed System One endpoint:

```sh
jev-observer --upstream https://openrouter.ai/api/v1/systemone
```

Register your OpenRouter key in **Connect an application**, which shows the configured endpoint and provider. Use the returned local client token in your application's SDK. Keep its base URL at `http://127.0.0.1:8765` and include `Accept-Encoding: identity`, as above. Use a Jev model such as `jev-1.13` in the request. OpenRouter is recognized from its upstream hostname; validated reported USD cost is shown with a provider-reported basis.

OpenRouter documents this endpoint as compatible with the TypeSafe request and response shapes. Inference is billed to your OpenRouter account. See its [System One SDK guide](https://openrouter.ai/docs/guides/community/typesafe-sdk) for the endpoint and model identifiers, and [Observer compatibility evidence](compatibility.md) for the exact tested scope.

## Laya and other local System One models

These options require **Observer 0.2.0 or later**. Laya exposes the same `POST /v1/systemone` route. Start its server separately, bound to loopback; Observer does not download or host the model. This setup was tested with `laya[serve]==0.3.23` and its English checkpoint:

```sh
# In a separate Python environment with laya[serve]==0.3.23 installed:
LAYA_HOST=127.0.0.1 LAYA_PORT=8000 LAYA_MODELS=english \
  LAYA_DEFAULT_MODEL=english LAYA_DEVICE=cpu laya-serve

# In another terminal, after setting the saved JEV_OBSERVER_DB_KEY:
jev-observer --upstream http://127.0.0.1:8000/v1/systemone \
  --upstream-auth none --provider laya
```

This example assumes `LAYA_API_KEY` is unset. Read the workspace access token into `JEV_OBSERVER_ACCESS_TOKEN` privately, then use it as the SDK's `api_key` / `apiKey`. Keep the SDK base URL at the Observer origin and `Accept-Encoding: identity`. Set the request model to `english`. CPU inference can exceed the SDKs' ten-second defaults: set Python's `timeout=180` (seconds) or JavaScript's `timeout: 180000` (milliseconds), then tune this for your model and machine. This also applies when Laya requires a provider key. The connection panel supplies matching examples and confirms the first saved request.

For models that need longer than Observer's default five-minute deadline, start it with a larger value such as `--upstream-timeout-secs 600` and increase the SDK timeout too. The shorter of the SDK and Observer deadlines limits the call.

`--upstream-auth none` requires a loopback upstream. Observer still authenticates every caller using its workspace access token and removes local authorization before forwarding. No provider-key registration is needed, and inherited `TYPESAFE_API_KEY` values are ignored. If Laya requires `LAYA_API_KEY`, keep the default bearer mode instead, register that key in Observer, and use the returned local client token.

The `laya` adapter handles its four-decimal probabilities, string-list or object Choice criteria, and one to 32 Score levels. It retains zero output-token usage and leaves unreported cost unknown. Extended numeric Choice labels, abstention-specific semantics, batch calls and non-System-One chat endpoints are outside this typed-capture scope. Other compatible local servers can use a descriptive `--provider` label; they use the standard Jev typed-answer validation. The upstream URL remains fixed for each Observer process.

See the [reviewed Laya HTTP server](https://github.com/NandhaKishorM/laya/blob/4aa6761be8173de4ce6d92c31b3e40b6eaf59a7c/laya/serve.py) and [real inference evidence](compatibility.md#live-laya-check) for tested versions and limits.

## Credential handling and metadata

Registration does not write the provider key or local client token to SQLite, exports, or the status API. SQLite stores only a hash of the random local token to approve restoring a saved system credential; replacing or removing the key revokes that approval even if the system store is locked. If a proxied request or response echoes either value, Observer redacts it from captured history. System storage is scoped to the workspace database path; session storage ends when Observer stops. The local token works for its registered workspace and is never forwarded to the provider. The environment fallback remains available for existing integrations when requests include the workspace access token. Leave `TYPESAFE_API_KEY` unset when you want to use only a registered key. Observer strips local request cookies, browser origin/referrer headers, and local forwarding metadata before forwarding; it ignores provider `Set-Cookie` headers.

The Basic authentication scheme is case-insensitive and accepts one or more ASCII spaces before the encoded credential. The credential itself is matched exactly, including case.

When a proxy request uses dashboard Basic authentication, Observer also redacts the accepted full header and its encoded credential component from captured bodies and local metadata. Known Basic authorization fields in imported data protect their full value and encoded component too. This uses exact matching without Base64 decoding; an ordinary string alone does not establish that it is a credential.

Optional local metadata headers are stripped before forwarding:

| Header | Purpose |
|---|---|
| `x-observer-source` | Separate applications' question groups |
| `x-observer-task-version` | Distinguish rule changes carried in input state |
| `x-observer-adapter: jgrep-v1` | Enable the narrowly matched, source-reviewed jgrep indexed-family adapter |

Grouping includes the source, original key, full definition, presentation order and supplied task version. Different definitions retain separate statistics. Missing task context is marked unverified; redacted definitions are conservatively isolated. See the [grouping rationale](../research/grouping.md).


See [storage, privacy and operating limits](storage.md) for encryption, migration and backups.
