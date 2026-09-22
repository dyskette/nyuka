One inconsistency to resolve first: the backend has no OTLP receiver (you chose `tracing` + JSON stdout, no collector). Two ways to honor "OTel web SDK to the backend":

- Recommended: backend exposes `POST /api/v1/telemetry` accepting OTLP/HTTP JSON, validates and rate-limits it, and emits each span/event as a `tracing` event → JSON stdout. Zero new containers; you get client errors and page timings in the same log stream. Later you can swap the handler to forward to a real collector.
- Alternative: add an `otel-collector` container to Compose and point both frontend and backend at it. Cleaner long-term, one more thing to run and secure (the collector endpoint must be reachable from browsers, so it needs auth via your session cookie anyway).

**Resolved in [ADR-0013](docs/adr/0013-ingest-frontend-telemetry-through-the-api.md):** the first option, with a phased frontend scope. Phase 1 ships hand-rolled `traceparent` propagation plus 100% error reporting (a few hundred bytes); the OpenTelemetry web SDK is Phase 2, lazy-loaded, because assembled carelessly it is 200 KB+ before first interaction.

Here's the frontend architecture.

## Frontend architecture — v1

### Decisions locked

| Area | Choice |
|---|---|
| Shell | Vite 8 + React 19 SPA, TypeScript strict, embedded in the Rust binary |
| Routing / data | TanStack Router (file-based, code-gen) + TanStack Query v5 |
| API client | `openapi-typescript` types + `openapi-fetch`; hand-written Query hooks per feature |
| Forms | TanStack Form + Zod 4 (Standard Schema; no `@tanstack/zod-adapter`) |
| UI | Tailwind v4 + shadcn/ui (Radix), lucide icons |
| Lists | TanStack Table v9 + TanStack Virtual |
| Live updates | One `EventSource` → invalidate/patch Query cache |
| i18n / theme | Lingui (ICU, compile-time); light/dark via `class` strategy + system preference |
| Quality | Biome; Vitest + Testing Library + MSW; Playwright E2E |
| Observability | Route error boundaries, `sonner` toasts, problem+json parsing, `traceparent` propagation → backend ingest endpoint (OTel web SDK deferred to Phase 2 — ADR-0013) |

### Structure

```
web/
  src/
    app/                 providers (Query, Router, i18n, theme, OTel), root layout, router.tsx
    routes/              TanStack file routes; thin: parse params, call loaders, render feature views
    features/
      sources/           api/ (hooks, query keys), components/, forms/, types.ts
      catalog/
      library/
      downloads/
      jobs/
      auth/              useMe(), login redirect, AuthGate
    shared/
      api/               client.ts (openapi-fetch instance), schema.d.ts (generated), problem.ts, sse.ts
      ui/                shadcn components (owned), layout primitives, DataTable, VirtualList
      lib/               cn(), formatters, date, query-client.ts, telemetry.ts
      i18n/              lingui config, catalogs/{en,es}/messages.po
    test/                setup, MSW handlers (generated from schema where possible), factories
  e2e/                   Playwright specs
  openapi.json           committed; source of truth from `cargo xtask openapi`
```

Rules: `routes/` imports from `features/`; `features/` import from `shared/` and never from each other (cross-feature composition happens in a route). `shared/ui` never imports API code.

### Data layer

- `client.ts`: `createClient<paths>({ baseUrl: '/api/v1' })` with middleware for: `X-Requested-With` CSRF header, `Accept: application/json`, problem+json → `ApiError` class, `401` → redirect to `/api/v1/auth/login?return_to=…`, `Retry-After` honored on 429/503.
- Query key factories per feature: `sourcesKeys.all`, `.detail(id)`, `.catalog(id, params)`; keys are typed and colocated with the hooks.
- Cursor pagination via `useInfiniteQuery` for catalog, chapters, jobs. ETags handled by the browser cache; Query `staleTime` tuned per resource (catalog 5 min, library 30 s, jobs 0 with SSE driving updates).
- Mutations: optimistic for follow toggles and job cancel; `Idempotency-Key` (uuid v7) generated per `POST /downloads` submit and reused on retry.
- Route loaders call `queryClient.ensureQueryData` so navigation is data-aware, with `pendingComponent` skeletons and `errorComponent` per route. Search params validated with Zod via `validateSearch` (filters, cursor, sort live in the URL).

### SSE integration

`shared/api/sse.ts` opens one `EventSource('/api/v1/events')` in a provider mounted after auth; reconnects with backoff, sends `Last-Event-ID`. Replay is a latency optimization only — correctness comes from invalidating the live-maintained queries on every reconnect (ADR-0010). Production needs HTTP/2, or roughly six open tabs exhaust the browser's per-origin connection pool and every request queues silently. Event handling table:

| Event | Cache action |
|---|---|
| `job.progress` | `setQueryData(jobsKeys.detail(id))` patch; update row in infinite list in place |
| `job.state` | patch + invalidate `jobsKeys.list()` counts; toast on `failed` |
| `chapter.new` | invalidate `libraryKeys.chapters(mangaId)` and follows badge |
| `chapter.downloaded` | patch chapter row, invalidate storage stats |
| `source.updated` | invalidate `sourcesKeys.all` |

No component touches the `EventSource`; they only read Query.

### Forms

TanStack Form with Zod 4 schemas passed directly via Standard Schema (do not add `@tanstack/zod-adapter`; it does not work with Zod 4); Zod schemas derived from generated types where possible (`z.object` shaped to `components['schemas']['FollowCreate']`, checked at compile time with `satisfies`). Source settings are dynamic: the backend returns a settings schema from the `.aix`; render a `DynamicForm` mapping Aidoku setting types (toggle, select, multi-select, text, stepper) to shadcn controls.

### UI / layout

Sidebar layout (`shared/ui/AppShell`): Sources, Browse, Library, Downloads, Jobs, Settings. Desktop-first at 1280+, collapsible sidebar at ≤1024, tablet at 768 with sidebar as sheet. Dense `DataTable` component: TanStack Table for columns/sort/selection + TanStack Virtual for rows; column visibility persisted in `localStorage`. Bulk selection on chapters → "Download now" / "Schedule" actions. Global command palette (`cmdk`) for search across sources. Theme: `next-themes`-style provider writing `class="dark"`, tokens as Tailwind v4 CSS variables matching shadcn.

### i18n

Lingui with `@lingui/vite-plugin`; source locale `en`, second locale `es` from the start so extraction and plural handling are exercised. Messages via macros (`t`, `<Trans>`, `plural`). Locale from user preference (settings) → `navigator.language` fallback; dates/numbers via `Intl`. Catalogs lazy-loaded per locale.

### Errors & telemetry

- `ApiError` carries `type`, `title`, `status`, `detail`, `instance`, `errors[]` (field errors map into TanStack Form).
- Root error boundary (crash page with "report" that attaches the OTel trace id), route boundaries for recoverable failures, mutation errors → toast with `title`/`detail`.
- Telemetry, Phase 1: a hand-rolled W3C `traceparent` per interaction, propagated to `/api/v1` so backend logs correlate, and uncaught exceptions posted to `/api/v1/telemetry` as OTLP/JSON at 100%. The generator must satisfy Trace Context Level 2 — set the random flag, 56 bits of entropy from `crypto.getRandomValues` (ADR-0015).
- Telemetry, Phase 2 (when page timings are wanted): `@opentelemetry/sdk-trace-web` with `StackContextManager` (never `@opentelemetry/context-zone`; `zone.js` is ~1 MB) plus document-load and fetch instrumentations, lazy-loaded outside the main chunk, traces sampled 10%. Enabled only with `VITE_OTEL=1`.

### Testing

- Unit/integration: Vitest (jsdom), Testing Library, MSW handlers typed against `paths` (via `openapi-msw`). Test routes with `createMemoryHistory`; test SSE handlers by injecting a fake `EventSource`.
- E2E: Playwright against the real Rust binary with a seeded Postgres and a stub OIDC provider (e.g. `oidc-provider-mock` container), covering: login, install source, browse and add to library, queue download and watch progress, schedule and cancel.
- Coverage targets: shared/api and features/*/api ≥ 90%, components ≥ 70%.

### Tooling & pipeline

- Biome (lint + format, `recommended` + `a11y` rules), `tsc --noEmit`, `lingui extract --clean` check, `knip` for dead exports, `size-limit` on the main chunk.
- Codegen: `cargo xtask openapi` writes `web/openapi.json`; `pnpm gen:api` runs `openapi-typescript` → `schema.d.ts`; CI fails if either is stale.
- Vite: `server.proxy['/api'] → http://localhost:8080` with `changeOrigin: false` so cookies work; `build.target: 'es2022'`, manual chunks for vendor/router/query; `rust-embed` picks up `web/dist` in the release build.
- CI stages (frontend job): install → biome → tsc → lingui check → vitest → build → Playwright (against the built binary from the backend job).
- Renovate for dependency updates, grouped by ecosystem.

### ADRs

Written up in [`docs/adr/`](docs/adr/README.md): 0008 routing and data, 0009 hand-written Query hooks, 0010 SSE into the Query cache, 0011 Lingui, 0012 Biome, 0013 frontend telemetry ingest.
