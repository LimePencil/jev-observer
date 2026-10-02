# Observer interface

React, TypeScript and Vite, with selective Radix primitives, Phosphor icons and locally bundled Geist fonts. Charts use SVG and include accessible data tables. No chart framework, remote assets, analytics or hosted frontend runtime.

```sh
npm ci
npm run dev
```

Run the local Observer backend on port 8765. The development server forwards `/api` to it; only its own same-origin development requests receive the corresponding backend Origin. Production requests use the backend's same-origin API directly.

```sh
npm run build
```

The backend embeds `dist/` when compiled. Build the interface before compiling the executable.

```sh
npx playwright install chromium
npm test
```

Set `OBSERVER_UI_PREVIEW=1` when running `npm test` after a UI build to check the production bundle through Vite preview. The default runs the development server.

Browser tests intercept API responses with explicitly synthetic fixtures. They verify accounting presentation, pause/resume behavior, independent collection health, review labels, version comparison, import failures, export downloads, deliberate deletion, and mobile navigation. They do not claim live provider compatibility or backend performance. The application contains no mock API fallback.

Dashboard and health polling use independent, bounded, sequential requests and pause in hidden browser tabs. Pausing the visible dashboard retains newer snapshots without stopping collection. Inspecting a record also holds the visible dashboard. Group and request lists are bounded by the server; the interface discloses the current feed limit.

History pages, source/model/search filters and custom local date ranges are saved in the page URL. Request and group pagination affect their lists; summary totals keep the full selected scope. Question search queries retained groups on the server. Timelines use timestamp spacing and server-supplied bucket bounds, including calendar dates across days.

Connection setup shows the configured provider and upstream, Python/JavaScript SDK examples with identity encoding, and collection status. A local upstream with `upstream_auth: "none"` uses the workspace access token to authenticate to Observer and offers no provider-key registration. Remote bearer mode uses the registered local client token. Examples read credentials from environment variables and never include entered secrets.

Version comparisons show separate observed samples: distributions, failures, request usage, latency, warnings and review coverage. They are not matched-input replay results. Provider-reported charges and configured estimates remain identified at request level; structurally valid Score answers retain any rubric warnings.

To exercise the embedded UI and real authenticated backend, build both UI and Rust, then run:

```sh
OBSERVER_BINARY="$PWD/../target/debug/jev-observer" npm run test:integration
```

Run this command from `ui/` (or supply an absolute executable path). Integration tests create and remove their own temporary demo and live encrypted workspaces. They make no provider calls.
