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

Browser tests intercept API responses with explicitly synthetic fixtures. They verify accounting presentation, pause/resume behavior, independent collection health, review labels, version comparison, import failures, export downloads, deliberate deletion, and mobile navigation. They do not claim live provider compatibility or backend performance. The application contains no mock API fallback.

Dashboard and health polling use independent, bounded, sequential requests and pause in hidden browser tabs. Pausing the visible dashboard retains newer snapshots without stopping collection. Inspecting a record also holds the visible dashboard. Group and request lists are bounded by the server; the interface discloses the current feed limit.
