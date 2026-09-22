# ADR-0008: Use TanStack Router and Query for routing and data

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/app`, `web/src/routes`, `web/src/features/*/api` |
| **Supersedes** | None |

This article explains why the SPA routes with TanStack Router and caches server state with TanStack Query, rather than using React Router. It also records the division of labor between the two libraries, because getting that boundary wrong produces two competing caches.

## Context

### The URL is the state container

Most of this application's screens are driven by state that belongs in the URL. The backend exposes `GET /sources/{id}/catalog?q=&filters=&cursor=`, `GET /jobs?state=&cursor=`, and cursor-paginated library and chapter listings. The frontend design puts filters, cursor, and sort in the URL so that a filtered catalog view is shareable, bookmarkable, and survives a refresh, and so that browser back and forward behave correctly without duplicated component state.

That means search parameters are not incidental — they are the primary input to the data layer. A filter string arriving from a bookmark or an edited URL is untrusted input that must be validated before anything reads it.

### Typing is end to end everywhere else

The API contract is generated: `cargo xtask openapi` produces `web/openapi.json`, `openapi-typescript` turns it into `schema.d.ts`, and CI fails when either is stale. TypeScript runs strict. In a codebase where request and response shapes are derived from the server's own schema, a router that types paths, params, and loader data completes the chain, and one that hands back `string` and `Record<string, string>` breaks it at exactly the boundary where URLs enter.

### There is no server runtime for the frontend

Per [ADR-0006](0006-embed-the-frontend-in-the-binary.md), `web/dist` is compiled into the Rust binary and served as static assets. There is no Node process in production. Any framework whose primary mode assumes a server is being used in its secondary mode here.

### Navigation must be data-aware

Route transitions should start their data fetches before the component renders, show a per-route skeleton while pending, and render a per-route error component on failure. Live updates arrive over SSE and patch or invalidate cached queries, as covered in ADR-0010. Tests drive routes with an in-memory history.

## Decision

Use **TanStack Router 1.170.x** for routing and **TanStack Query 5.103.x** for server-state caching.

- **File-based routes with generated types.** `@tanstack/router-plugin` for Vite maps `src/routes/` to a route tree and writes `routeTree.gen.ts`. Commit that file and add a CI check that fails when it is stale, alongside the existing `openapi.json` check.
- **Validate search parameters with Zod 4 schemas passed directly to `validateSearch`.** Zod implements Standard Schema, so TanStack Router consumes the schema without an adapter.

  > [!IMPORTANT]
  > Do not add `@tanstack/zod-adapter`. It does not work with Zod 4; passing the schema directly is both the supported path and less code. Any tutorial showing `zodValidator` around a search schema predates this.

- **Loaders prime the cache; they do not fetch.** A route loader calls `queryClient.ensureQueryData` with the same query key the component's hook uses, and returns nothing the component reads. Components always read through `useQuery` or `useInfiniteQuery`.

  > [!IMPORTANT]
  > This is the boundary that keeps the two libraries from fighting. If a loader returns data and a component reads it through `Route.useLoaderData()`, that data lives in the router's cache while the same resource also lives in Query's cache — and an SSE-driven `setQueryData` patch updates one of them. Router decides *when and where*; Query owns *what*, including caching, staleness, and invalidation.

- **Per-route boundaries.** Each route declares `pendingComponent` and `errorComponent`. Problem+json parsing and `ApiError` mapping stay in the fetch middleware, not in route code.
- **Pin and group.** TanStack Router publishes patch releases almost daily. Pin exact minor lines and group TanStack packages into one Renovate rule so upgrades are a single reviewed change.

### Toolchain versions this assumes

The architecture draft names Vite 6 and an unversioned Zod and TanStack Table. The current baselines differ, and the ADR records them because the routing choice interacts with the build:

| Package | Draft | Current (Sept 2026) |
|---|---|---|
| Vite | 6 | **8.3.0** — Rolldown-based; `manualChunks` object form is gone, replaced by `output.codeSplitting.groups` of `{ name, test }` |
| React | 19 | 19.3.0 |
| TypeScript | unversioned | **5.9.3** — 7.0.2 is the latest release, but every version of `openapi-typescript` peers on `typescript@^5.x`, and that tool generates the API contract CI checks for staleness. Verified by install. |
| Zod | unversioned | **4.6.5** — Standard Schema, no adapter |
| TanStack Table | unversioned | **9.2.4** |
| TanStack Router | — | 1.170.38, requires Node ≥ 20.19 |
| TanStack Query | v5 | 5.103.2 — v5 is still current |

## Consequences

### What you gain

- **Invalid URLs fail at the type boundary and at runtime.** `validateSearch` parses and defaults search params before a loader or component sees them, and `useSearch()` returns a typed object. A hand-edited `?state=nonsense` becomes a validation error at the route, not an unexpected value inside a component.
- **Paths, params, search, and loader data are typed without manual annotation.** `Link` targets are checked, so a renamed route breaks the build rather than producing a dead link found by a user.
- **The URL carries the state, once.** Filters and cursors are not mirrored in component state, which removes the whole category of bugs where the URL and the UI disagree.
- **Navigation is data-aware by default.** `ensureQueryData` in the loader means the fetch is in flight before render, with a per-route skeleton while it lands.
- **The SSE integration has one target.** Because Query owns all server state, the event handlers in `shared/api/sse.ts` call `setQueryData` and `invalidateQueries` and nothing else. No component subscribes to the `EventSource`, and the router is not involved.
- **No unused framework surface.** No SSR, no server loaders, no route-level server actions — none of which could run in the deployment from ADR-0006.

### What it costs you

- **A second generated artifact.** `routeTree.gen.ts` joins `schema.d.ts` as a committed build product with a staleness check. Both are cheap individually; the pattern needs to stay disciplined or the checks get skipped.
- **The loader/Query boundary is a convention, not a compile error.** Nothing stops a future contributor from returning data from a loader and reading it in the component. It needs a written rule, a review habit, and ideally a lint rule; otherwise the duplicate-cache bug appears months later as "SSE updates sometimes don't show".
- **High release cadence.** Patch releases land almost daily on the 1.x line. They are non-breaking, but without pinning and grouping the dependency noise is constant.
- **A smaller answer pool.** React Router has far more existing answers, tutorials, and developers who already know it. For a small team this is a real onboarding cost.
- **No path to SSR within this library.** If server rendering is ever required, that is a move to TanStack Start or a different framework, not a configuration change.

### Follow-up work this decision creates

1. **Add the `routeTree.gen.ts` staleness check** to the frontend CI job next to the OpenAPI check.
2. **Write the loader convention down** in `web/README.md` and enforce it in review: loaders call `ensureQueryData` and return nothing; components read through hooks.
3. **Define the search-schema-to-query-key rule** so that a validated search object maps to a query key deterministically. This is what makes cache invalidation predictable across filtered views.
4. **Set up the memory-history test harness** once, in `test/`, so route tests do not each reinvent it.
5. **Group TanStack packages in Renovate** and pin exact minors.
6. **Correct the toolchain versions** in the frontend architecture document to the table above, so the Vite 6 reference does not propagate into setup instructions.

## Alternatives considered

| Alternative | Search param typing | SSR assumption | Outcome |
|---|---|---|---|
| TanStack Router + Query | Typed and validated by default | None | **Chosen** |
| React Router v8 | Untyped by default | Framework mode assumes a server | Rejected, on typing depth for this URL-driven UI. |
| React Router v8, data mode only | Untyped by default | None | Rejected. Same typing gap without the framework benefits. |
| Next.js, Remix, or TanStack Start | Varies | Yes | Rejected. No Node runtime in production. |
| A minimal router such as `wouter` | None | None | Rejected. Too little for validated search state and per-route boundaries. |

### React Router v8

React Router is the stronger project on process, and that deserves to be said plainly rather than buried. Version 8.0.0 shipped on 17 June 2026 as the first major release under a formal open governance model with a **predictable annual release cycle**, and it was deliberately unexciting: breaking changes were introduced ahead of time behind future flags, so a v7 application that had adopted the flags needed little work. The three real changes were the removal of the `react-router-dom` package, ESM-only builds, and raised baselines (Node 22.22+, React 19.2.7+, Vite 7+). Version 8.4.0 is current, and the v7 line still receives security updates while v6 and Remix v2 are end of life.

Compared with TanStack Router's continuous 1.170.x stream, an annual major with future-flag migration is the more predictable model for a long-lived project, and if release predictability were the deciding criterion, React Router would win.

It was rejected because the deciding criterion is typing at the URL boundary:

1. **Search params are untyped by default.** React Router v7 added typed routes via a `routes.ts` config and generated `+types/` directory, and v8 carries that forward, but paths remain string-typed in many positions and search params are something you type yourself. In an application where filters, sort, and cursor drive every list endpoint, that is the one place hand-written types will drift from reality.
2. **Framework mode assumes a server.** Its data mutations, server loaders, and middleware — enabled by default in v8 — target a deployment that does not exist here. Using the data or declarative mode instead means adopting a framework and then declining most of it.
3. **The toolchain baseline is prescriptive.** ESM-only and Vite 7+ are fine in isolation, but they mean the router dictates build-tool floors for a project that otherwise has free choice.

This is the alternative to reconsider if the typing benefit turns out not to pay for the smaller ecosystem in practice.

### Next.js, Remix, or TanStack Start

Rejected on deployment, not on merit. ADR-0006 compiles the frontend into the Rust binary and serves it as static files; there is no Node process in production and no intent to add one. Server rendering would also complicate the same-origin cookie authentication from ADR-0005 by introducing a second thing that holds a session.

### A minimal router

`wouter` and similar libraries are excellent when routing is path matching. Here it also has to validate search schemas, drive loaders, and host per-route pending and error boundaries, which would mean rebuilding most of what was rejected.

## Revisit this decision when

- **Server rendering or SEO becomes a requirement.** Neither applies to an authenticated self-hosted tool today, but both would invalidate the "no server runtime" premise and reopen the framework question first.
- **The loader/Query boundary is violated in practice** despite the written rule, producing stale-view bugs. That would argue for collapsing to one data layer rather than for changing routers.
- **The release cadence becomes a maintenance burden** that pinning and grouping do not contain.
- **TanStack Router's typing advantage narrows.** React Router has been closing this gap across v7 and v8; if search-param inference reaches parity, the ecosystem-size argument becomes decisive in the other direction.

## References

- [TanStack Router: search params guide](https://tanstack.com/router/latest/docs/framework/react/guide/search-params)
- [TanStack Router: validate search params with schemas](https://tanstack.com/router/latest/docs/framework/react/how-to/validate-search-params)
- [TanStack Router on npm](https://www.npmjs.com/package/@tanstack/react-router) — 1.170.38
- [TanStack Query on npm](https://www.npmjs.com/package/@tanstack/react-query) — 5.103.2
- [React Router v8 release announcement](https://remix.run/blog/react-router-v8)
- [React Router v8: ESM-only builds and default middleware](https://www.infoq.com/news/2026/08/react-route-v8/) — directional
- [React Router changelog](https://reactrouter.com/changelog)
- [Standard Schema](https://standardschema.dev/)
