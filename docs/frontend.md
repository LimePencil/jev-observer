# Frontend structure

`ui/src/App.tsx` is the composition root. It creates workspace owners once and
connects the shell, dashboard, and lazy detail drawer. Presentation components
receive their state and actions through typed props; they do not start writes.

- `app/useAppShell.ts` owns theme persistence, navigation, keyboard shortcuts,
  notifications, and drawer focus restoration. Opening a new panel cancels any
  pending restoration frame. Closing a panel tries its opener, then the main
  landmark if the opener cannot receive focus.
- `app/useDashboardWorkspace.ts` owns URL filters, draft search debounce,
  pagination transactions, the displayed snapshot, and history revisions.
  Pagination enters history only after its request succeeds. Pausing or opening
  details holds the visible snapshot while collection and health polling continue.
- `app/useWorkspaceControllers.ts` mounts review, credential, import, and deletion
  controllers for the workspace lifetime. Closing a drawer does not cancel or
  duplicate an in-flight write. The existing controllers retain their generation
  and cancellation guards.
- `app/WorkspaceShell.tsx`, `DashboardFilters.tsx`, and `DashboardContent.tsx`
  render the shell, filter controls, and loaded dashboard without owning transport
  or mutation state.
- `Details.tsx` remains the lazy-loaded drawer entry point. `details/` contains
  one screen per task and shared drawer fields/headings. Screen-local state, such
  as an import draft or comparison selection, stays with that screen. Request
  detail keys and the existing drawer mount boundaries remain deliberate.
- `components/` separates shared primitives, charts, and request tables.
  `components.tsx` preserves the public import surface for views and tests.

The smaller modules remain focused: `api.ts` handles transport and polling,
`format.ts` handles display formatting, `connection.ts` builds SDK examples, and
`ModelCatalog.tsx` renders provider capabilities. Dashboard and health polling
share request-order arbitration but intentionally have different deadlines and
failure behavior; their loops are not interchangeable.

Run `npm run build` and `OBSERVER_UI_PREVIEW=1 npm test -- --workers=1` in `ui/`
to validate the production bundle. The browser suite covers pagination rollback,
filter races, drawer loading failures, keyboard/focus behavior, accessibility,
responsive layout, and writes that complete after a drawer closes.
