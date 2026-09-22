# ADR-0017: Address the detail panel with a nested route

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/routes`, `web/src/shared/ui/AppShell` |
| **Supersedes** | None |

This article settles the choice the design specification leaves open: whether the master–detail panel is addressed by a search parameter (`?selected=<id>`) or by a nested route (`/library/$mangaId`). It picks the nested route, and the reason is that route-level loaders and boundaries come with it.

## Context

The design specification describes a three-column shell — sidebar, master list, detail panel — and states that the panel "opens on row select, URL `?selected=<id>` (or nested route `/library/$mangaId`)". Both forms are listed; neither is chosen. The associated behaviors are fixed either way:

- `Esc` closes the panel.
- `[` and `]` cycle resize presets.
- Below 1024 px the panel takes the full content area with a back button.
- The list is virtualized, with `j`/`k` navigation, multi-select, and bulk actions.

### The list's own state is substantial and must survive opening the panel

The master list holds a validated search object — filters, sort, and cursor — per [ADR-0008](0008-use-tanstack-router-and-query-for-routing-and-data.md), plus an infinite query whose accumulated pages and a virtualizer's scroll offset exist only in memory. A user who scrolls 400 rows into a filtered catalog, opens a detail panel, and closes it must land back where they were. Losing that is the most noticeable possible regression in this UI.

### Route-level infrastructure already exists and is unused by a search param

ADR-0008 established that every route declares `pendingComponent` and `errorComponent`, and that loaders prime the Query cache with `ensureQueryData` so navigation is data-aware. Those are **route** features. A panel driven by a search parameter is not a route, so it gets none of them: its loading skeleton, its error state, and its fetch trigger are all hand-written inside a component.

### TanStack Router keeps the parent mounted

This is the fact that decides the trade. When navigating to a child route, the parent route stays mounted and the child renders into its `<Outlet />`. Parent-level search parameters are preserved across that navigation using the `retainSearchParams` search middleware, which takes either `true` or an explicit list of keys.

So a nested route does not unmount the list, and does not discard its filters.

## Decision

Address the detail panel as a **nested route** under the list route.

- **Route shape.** `/library` validates the list's search parameters and renders the shell, the toolbar, and the virtualized table, with an `<Outlet />` in the detail column. `/library/$mangaId` renders the panel.
- **Retain list state across panel navigation.** Declare the list's filters, sort, and cursor in `validateSearch` on the **parent** route, and apply `retainSearchParams` as search middleware so opening or closing the panel never drops them.
- **The panel is a route, so it uses route features.** `loader` calls `ensureQueryData` for the manga detail; `pendingComponent` renders the panel-shaped skeleton after the specified 150 ms delay; `errorComponent` renders the panel's error state. None of this is hand-rolled.
- **Panel tabs are child routes.** Overview, Chapters, and Jobs become `/library/$mangaId/`, `/library/$mangaId/chapters`, and `/library/$mangaId/jobs`, so a tab is deep-linkable and each gets its own loader and boundaries.
- **`Esc` navigates to the parent route.** Closing the panel is `navigate({ to: '/library', search: prev => prev })`, not clearing a parameter.
- **Ephemeral panel state stays out of the path.** Resize preset and active density are per-viewer preferences in `localStorage`, not URL state. Only identity and tab are addressable.
- **Below 1024 px the same route renders full-width** with a back button. The breakpoint changes presentation, not addressing — which is only true because the route is the panel's identity.
- **Apply the same pattern in Browse, Downloads, and Jobs**, so master–detail is one shape across the application rather than three.

> [!IMPORTANT]
> `retainSearchParams` on the parent route is what makes this safe. Without it, opening the panel drops the list's filters and cursor from the URL, the parent's `validateSearch` re-defaults them, and the user's filtered view resets — while the list component stays mounted, so the symptom looks like a data bug rather than a routing one. Add it in the same commit as the nested route.

## Consequences

### What you gain

- **Loaders, pending, and error states for free.** The panel's data fetch starts before render, its skeleton is declared not coded, and a failed detail fetch is contained by a boundary rather than breaking the page.
- **The list keeps its state.** Parent stays mounted, so virtualizer offset and accumulated infinite-query pages survive; `retainSearchParams` keeps filters in the URL.
- **Tabs are addressable.** "Look at this manga's chapters" is a link, which matters for the crash-report and support paths.
- **One shape, four screens.** Library, Browse, Downloads, and Jobs share the pattern, so the shell and the keyboard map are written once.
- **Responsive behavior is presentational.** The same route renders as a panel at 1280 and full-width below 1024, because addressing does not depend on layout.
- **Type safety on the identity.** `$mangaId` is a typed path parameter; a wrong link fails the build rather than rendering an empty panel.

### What it costs you

- **More route files.** One list route becomes a layout route plus a detail route plus one per tab. File-based routing makes this cheap but it is more structure to navigate.
- **A generated route tree that grows.** `routeTree.gen.ts` is already a committed artifact with a staleness check per ADR-0008; this adds entries to it.
- **`retainSearchParams` is a trap if forgotten.** Described in the callout above, and the reason it is a follow-up test rather than a note.
- **Closing the panel is a navigation, not a state change.** It pushes history, so `Esc` then Back is a sequence to think about. Use a replace navigation where the panel open/close should not accumulate history entries.
- **Panel presentation is coupled to the parent's layout.** The detail column lives in the parent's JSX, so the parent must reserve space for an `<Outlet />` that is often empty — which is fine here, since the three-column shell is fixed, but it would not suit a layout where the panel is incidental.

### Follow-up work this decision creates

1. **Test that list state survives a panel round trip**: filter, scroll deep into a virtualized list, open the panel, close it, and assert filters, cursor, and scroll offset are unchanged. This is the regression this ADR exists to prevent.
2. **Add `retainSearchParams` on every master route** and assert in a test that opening a detail route preserves the parent's search object.
3. **Decide push versus replace** for panel open and close, and write it down. Row-to-row navigation within the panel probably wants replace; opening from the list probably wants push.
4. **Wire `Esc` to a parent navigation** through the central shortcut map in `shared/lib/shortcuts.ts`, so the palette hint and the handler cannot drift.
5. **Confirm the ≤1024 full-width rendering** uses the same route and that the back button is a parent navigation, not `history.back()`.

## Alternatives considered

| Alternative | Panel gets loaders and boundaries | List state preserved | Outcome |
|---|---|---|---|
| Nested route `/library/$mangaId` | Yes | Yes, with `retainSearchParams` | **Chosen** |
| Search param `?selected=<id>` | No, hand-rolled | Yes, trivially | Rejected. Reimplements route features. |
| Client state only, not in the URL | No | Yes | Rejected. Panel stops being linkable. |
| A modal or sheet at all widths | Depends | Yes | Rejected. Conflicts with the three-column design. |

### The search parameter `?selected=<id>`

The alternative the specification lists first, and it has genuine advantages: one route file, no route tree growth, list state preserved without any middleware, and closing the panel is a search update rather than a navigation. For a panel that is a lightweight view mode of a list, it is the simpler model.

It was rejected because the panel here is not lightweight. It loads a manga's detail, its chapter list, and its job history across three tabs, each of which needs a fetch, a skeleton, and an error state. With a search parameter, all of that is hand-written inside a component: a `useQuery` gated on the parameter's presence, a manually placed skeleton with its own 150 ms delay, and an error branch in JSX. Those are exactly the three things a route declaration provides, and ADR-0008 already committed to using them everywhere else. Choosing the search parameter means the most complex view in the application is the one that opts out of the router.

The tab problem compounds it: `?selected=42&tab=chapters` is two coupled search parameters whose valid combinations the schema cannot express well, versus a route per tab that cannot be in an invalid state.

### Client state only

Keeping the selected ID in a component or a store is the least code and loses the property the whole design is built on — a shareable, bookmarkable, refresh-surviving URL. Rejected immediately; recorded because it is the path of least resistance during implementation and should be visibly ruled out.

### A modal or sheet at every width

Rejected because it contradicts the three-column layout. The design specification is explicit that the panel sits beside the list at 1280 and up and only becomes full-width below 1024, and a modal would also break the specification's requirement that focus is not obscured by the sticky header or status bar.

## Revisit this decision when

- **A panel appears whose content is trivial** — a one-field preview with no fetch. A search parameter would be the right tool there, and mixing the two patterns is acceptable if the reason is written down.
- **History behavior becomes confusing** in user testing, particularly `Esc` followed by Back. That is a push-versus-replace fix, not a re-architecture.
- **The route tree becomes unwieldy** across four master–detail screens with tabs. The response is shared layout routes, not a change of addressing.
- **A second panel needs to be open at once**, such as the `o` shortcut's "open in new pane". A single `<Outlet />` cannot express two simultaneous details, and that would need either parallel routes or a search parameter for the secondary pane.

## References

- [TanStack Router: search params](https://tanstack.com/router/latest/docs/framework/react/guide/search-params)
- [TanStack Router: `retainSearchParams`](https://tanstack.com/router/v1/docs/api/router/retainSearchParamsFunction)
- [TanStack Router: sharing search params across routes](https://tanstack.com/router/latest/docs/how-to/share-search-params-across-routes)
- [Search params are state](https://tanstack.com/blog/search-params-are-state) — directional
- [ADR-0008](0008-use-tanstack-router-and-query-for-routing-and-data.md) — route loaders, `pendingComponent`, `errorComponent`
