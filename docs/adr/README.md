# Architecture decision records

This directory holds the architecture decision records for the project. Each record states one decision, the constraints that forced it, what it costs, and what would make it wrong.

Records are numbered in the order decisions were made, not in dependency order. Use the reading order below when onboarding.

## How to use these

- **Adding a record:** copy [`adr-template.md`](adr-template.md), take the next number, and keep the house style. The style rules are in an HTML comment at the bottom of the template.
- **Changing a decision:** write a new record that supersedes the old one. Do not edit an accepted record's Decision section — set its status to `Superseded` and link forward.
- **A record's follow-up list is work.** Several of these ADRs create tests and CI checks that are the only thing enforcing the decision. They are called out per record below where they are load-bearing.

## Index

| # | Decision | Status | In short |
|---|---|---|---|
| [0001](0001-use-axum-for-the-http-api.md) | Use Axum for the HTTP API | Accepted | Tower middleware and a `Send`, multi-threaded runtime match the WASM and job-engine design; Actix's `actix-rt` does not, and its SSE support is not in core. Pin `0.8`. |
| [0002](0002-use-seaorm-for-persistence.md) | Use SeaORM for persistence | Accepted | Async-native on SQLx 0.9 with declarative relations. Cost: no compile-time SQL verification, so a CI schema-drift check is the control that makes it safe. |
| [0003](0003-run-the-job-queue-in-postgres-and-in-process.md) | Run the job queue in PostgreSQL, in process | Accepted | Transactional enqueue with no outbox, per-source rate limiting that actually bounds, and SSE fan-out over an in-process channel. Assumes exactly one instance. |
| [0004](0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md) | Use Aidoku WASM sources as the provider mechanism | Accepted, with scoped capability gaps | Sandboxed third-party sources on `wasmtime` 48 LTS. The ABI is unversioned, so pin a commit SHA and build a conformance source. `canvas` and the webview imports are declared gaps. |
| [0005](0005-act-as-the-oidc-client-with-server-side-sessions.md) | Act as the OIDC client, with server-side sessions | Accepted | The BFF pattern per RFC 10017, chosen because `EventSource`, downloads, and `<img>` cannot send headers. Requires writing the session store and a documented `rsa` advisory exception. **Amended:** `AUTH_MODE=none` serves every request as one seeded local user, for a barebones self-host. |
| [0006](0006-embed-the-frontend-in-the-binary.md) | Embed the frontend in the binary, on one origin | Accepted | One artifact, one version, no CORS. Cost: frontend changes invalidate the Rust build. |
| [0007](0007-store-the-library-as-cbz-files-on-a-local-volume.md) | Store the library as CBZ files on a local volume | Accepted | Interoperability with Komga, Kavita, and offline readers is the point. One library root in v1; atomic rename requires the temp dir inside the root. |
| [0008](0008-use-tanstack-router-and-query-for-routing-and-data.md) | Use TanStack Router and Query for routing and data | Accepted | Validated, typed search params are the deciding factor for a URL-driven UI. Loaders prime the cache and return nothing components read. |
| [0009](0009-hand-write-query-hooks-with-semantic-key-factories.md) | Hand-write Query hooks with semantic key factories | Accepted | Resource-first keys, because the SSE contract invalidates across endpoints. Generated hooks expose keys but shape them by path. |
| [0010](0010-drive-live-state-through-one-sse-stream-into-the-query-cache.md) | Drive live state through one SSE stream into the Query cache | Accepted | One `EventSource`, handlers touch only the cache, correctness from invalidate-on-reconnect rather than replay. **Needs HTTP/2 in production.** |
| [0011](0011-use-lingui-for-internationalization.md) | Use Lingui for internationalization | Accepted | Compile-time extraction makes a missing translation a build failure. Use the Babel preset; the SWC plugin is experimental. |
| [0012](0012-use-biome-for-linting-and-formatting.md) | Use Biome for linting and formatting | Accepted | One tool, no plugin version matrix, and GritQL plugins can express the project's custom rules. Lint `a11y` does not discharge WCAG — `axe-core` does. |
| [0013](0013-ingest-frontend-telemetry-through-the-api.md) | Ingest frontend telemetry through the API | Accepted, with a phased frontend scope | An endpoint instead of a browser-reachable collector. Phase 1 is hand-rolled `traceparent` plus error reporting; the 200 KB OTel web SDK is Phase 2. |
| [0014](0014-keep-backend-telemetry-as-structured-logs.md) | Keep backend telemetry as structured logs | Accepted | No collector: an LGTM stack floors at 8 GB RAM. Keep the OTel layer without an exporter so trace IDs still correlate. Pin OTel crates to `0.32`. |
| [0015](0015-adopt-opentelemetry-semantics-as-the-instrumentation-model.md) | Adopt OpenTelemetry semantics as the instrumentation model | Accepted | `tracing` bridged to OTel, stable semantic conventions, W3C Trace Context. Read this before 0013 and 0014. |
| [0016](0016-use-a-single-accent-neutral-color-system.md) | Use a single-accent neutral color system | **Accepted, with required token corrections** | One accent hue; semantics as dots and bars, never fills. Measurement found the focus ring at 1.92:1 in light mode and three other failing tokens. |
| [0017](0017-address-the-detail-panel-with-a-nested-route.md) | Address the detail panel with a nested route | Accepted | `/library/$mangaId` over `?selected=`, because the parent stays mounted and the panel gets route loaders and boundaries. Needs `retainSearchParams`. |
| [0018](0018-keep-motion-in-css.md) | Keep motion in CSS | Accepted | No animation library: `@starting-style` and `allow-discrete` cover enter and exit. Use `tw-animate-css`; `tailwindcss-animate` is deprecated. |
| [0019](0019-define-the-two-service-level-objectives.md) | Define the two service level objectives | Accepted | 99.9% service-attributable job success; 95% of user-requested downloads readable within 60s. Source faults excluded and counted separately. Settles `NOTIFY` as unnecessary and keeps `vector`/Loki unbuilt. |
| [0020](0020-return-read-models-from-list-endpoints.md) | Return read models from list endpoints | Accepted | Four screens rendered lists of foreign keys. List endpoints return projections carrying their joined values; entity DTOs stay honest. Never cast an untrusted value in SQL — `AND` does not guard it. |
| [0021](0021-separate-the-server-data-directory-from-the-library.md) | Separate the server's data directory from the library | Accepted | `DATA_DIR` is required and may not overlap `LIBRARY_ROOT`. Holds installed source packages, so a restart does not leave every source listed and unusable. Backups and syncs must not carry third-party executable code. |

## Reading order

**Backend foundations.** 0001 (framework) → 0002 (persistence) → 0003 (jobs) → 0004 (providers) → 0005 (auth) → 0007 (storage).

**API shape.** 0020 (read models) sits between the backend and the frontend: it is why a list endpoint's response is not the entity behind it. Read it after 0002 and before 0009.

**Frontend foundations.** 0006 (delivery) → 0008 (routing and data) → 0009 (hooks) → 0010 (live updates).

**Observability.** 0015 (instrumentation model) → 0014 (backend sink) → 0013 (frontend ingest) → 0019 (objectives). In that order; 0015 is numbered last but underpins the other two, and 0019 closes what 0003 and 0014 deferred.

**Design system.** 0016 (color) → 0017 (master–detail) → 0018 (motion).

## Cross-cutting constraints

Several records depend on the same assumptions. Changing any of these means revisiting more than one ADR.

| Assumption | Records that depend on it |
|---|---|
| **Exactly one API instance.** The per-source semaphore, the SSE broadcast channel, and stale-lock recovery all require it. | 0003, 0010, 0014 |
| **Two writable volumes.** The library is what an operator backs up and syncs; the server's own state must not travel with it. | 0007, 0021 |
| **SQLx 0.9, via SeaORM 2.x.** This is why the job queue and the session store are hand-written: every off-the-shelf crate is on SQLx 0.8. | 0002, 0003, 0005 |
| **Same origin, cookie authentication.** Follows from `EventSource` being unable to send headers. | 0005, 0006, 0010 |
| **No data leaves the deployment.** Telemetry describes reading habits, so hosted backends and error-tracking SaaS are the operator's choice, not the project's default. | 0013, 0014, 0015 |
| **HTTP/2 in production.** An SSE stream occupies one of six HTTP/1.1 connections per origin, and the seventh request of any kind queues silently. | 0010, and the deployment runbook |

## Load-bearing follow-ups

These are the checks without which the corresponding decision is unsafe rather than merely unenforced:

- **0002** — CI schema-drift check: run migrations on an ephemeral database, regenerate entities, fail on diff.
- **0003** — a concurrency test asserting each job row is claimed exactly once, plus stale-lock recovery.
- **0004** — a conformance `.aix` built at the pinned aidoku-rs SHA, exercising every implemented host import.
- **0005** — a documented `cargo-deny` exception for RUSTSEC-2023-0071 with reasoning, owner, and review date.
- **0007** — the temp directory inside the library root, asserted by test.
- **0009** and **0012** — the GritQL rule banning inline query-key arrays.
- **0010** — a test asserting `text/event-stream` responses are never compressed.
- **0016** — a contrast test over *rendered* colors, in both themes, including the focus ring. **Done**, in `web/e2e/contrast.spec.ts`. It measures by painting each token to a canvas and reading the pixel back, which is the sRGB the browser produces after gamut mapping; compositing a token over its *own background* rather than over black is what makes an alpha value measure as it looks. It found a live failure the token table could not: the sidebar's shortcut hints were `--muted-foreground` under `opacity-60`, at 2.54:1.
- **0019** — the hourly `nyuka_metrics` snapshot line, which is the only thing that makes the objectives evaluable after log rotation.
- **0020** — a database test per read model asserting its aggregates against a known fixture, *including the zero case*. An `INNER JOIN` written where a `LEFT JOIN` was meant passes every test that has a non-zero answer.
- **0021** — `tests/restart.rs`: a package saved by one runtime must load into a fresh one, and `install` must register under the id the database returned. Both bugs were invisible to tests that install and read in the same process, which is what every existing test did.
