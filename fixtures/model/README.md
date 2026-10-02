# Model fixtures

`jevrouter-receipt.json` is synthetic data following the real JevRouter `RouteResult` receipt shape at [commit f944acb](https://github.com/BillionsBobby/JevRouter/blob/f944acb6530621bced023352e2358a63218bf4d9/src/types.ts). JevRouter's [saveDecision](https://github.com/BillionsBobby/JevRouter/blob/f944acb6530621bced023352e2358a63218bf4d9/src/store.ts) writes that object directly. It is not an observed model call or execution result.

Import it with format `jevrouter-receipt`. The adapter accepts a single JSON object, an array, or JSONL, up to 10,000 records. Plans are not supported. A receipt is retained as an application action with its explicit decision ID, policy decision, execution state, raw provider envelope and provenance. It does not increment inference request, token or cost totals. The example keeps the provider's choice separate from the router's selected action.

Original HTTP status, timing, timestamp and full question definitions are absent here and remain unknown. `imported_at` records import time, with `timestamp_basis: "import"`. Application input is removed unless state capture is enabled. `observer-jsonl` can export/reimport these records; grouping fingerprints and validity are recomputed, and storage deduplicates explicit source event IDs.
