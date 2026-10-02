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

Open the local dashboard with username `observer` and the token in `.jev-observer/observer.access-token`, whose path Observer prints at startup. This owner-only token protects local history and key settings from other accounts on the same computer. Keep it private. SDK requests using a registered local client token need no additional header; SDK requests using their provider key directly must also send this access token in `x-observer-access`.

Run it from your application's directory to keep history in that project's `.jev-observer/` folder, or choose a stable location with `--db /path/to/observer.sqlite`. For a source build, use `./target/release/jev-observer` instead.

Point your application's SDK at the local **origin**, without appending `/v1/systemone`. The tested clients append that path themselves. Keep your existing credentials in the application environment.

Alternatively, open **Connect an application** in the dashboard, enter your provider key, and choose session-only storage or your operating system's credential store. Observer shows a local client token once. Put that token in the SDK's `api_key` / `apiKey` setting instead of the provider key. Observer validates the token and substitutes the registered provider key before forwarding. Keep the token private; losing it requires registering the provider key again to rotate the token. An unavailable or locked system credential store returns an error; session-only storage remains available.

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

Native forwarding covers `POST /v1/systemone`; the default destination is `https://api.typesafe.ai/v1/systemone`. Configure a different fixed endpoint with `--upstream URL`. Remote upstreams require HTTPS; HTTP is accepted only for a loopback mock. Caller authorization takes precedence over an optional `TYPESAFE_API_KEY` fallback in Observer's environment. Direct provider-key and environment-fallback requests must send the dashboard token in `X-Observer-Access`; the fallback also requires an `application/json` Content-Type. Provider keys cannot be passed as command-line arguments. Observer adds no upstream retries, caching or redirects.

Registration does not write the provider key or local client token to SQLite, exports, or the status API. SQLite stores only a hash of the random local token to approve restoring a saved system credential; replacing or removing the key revokes that approval even if the system store is locked. If a proxied request or response echoes either value, Observer redacts it from captured history. System storage is scoped to the workspace database path; session storage ends when Observer stops. The local token works for its registered workspace and is never forwarded to the provider. The environment fallback remains available for existing integrations when requests include the workspace access token. Leave `TYPESAFE_API_KEY` unset when you want to use only a registered key. Observer strips local request cookies, browser origin/referrer headers, and local forwarding metadata before forwarding; it ignores provider `Set-Cookie` headers.

Optional local metadata headers are stripped before forwarding:

| Header | Purpose |
|---|---|
| `x-observer-source` | Separate applications' question groups |
| `x-observer-task-version` | Distinguish rule changes carried in input state |
| `x-observer-adapter: jgrep-v1` | Enable the narrowly matched, source-reviewed jgrep indexed-family adapter |

Grouping includes the source, original key, full definition, presentation order and supplied task version. Different definitions retain separate statistics. Missing task context is marked unverified; redacted definitions are conservatively isolated. See the [grouping rationale](../research/grouping.md).


See [storage, privacy and operating limits](storage.md) for encryption, migration and backups.
