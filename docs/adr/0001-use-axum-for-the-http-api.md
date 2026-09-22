# ADR-0001: Use Axum for the HTTP API

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/api` |
| **Supersedes** | None |

This article explains why the `manga-api` HTTP layer is built on [Axum](https://github.com/tokio-rs/axum) rather than Actix Web, what you gain from that choice, and what it costs you to maintain.

## Context

`manga-api` ships as a single Rust binary that serves a REST API, a Server-Sent Events (SSE) stream, and the embedded React single-page application from one origin. The following constraints were already fixed before the framework was selected, and they narrow the field more than any general-purpose framework comparison does.

### The runtime is multi-threaded and every task must be `Send`

Three subsystems share the process runtime:

- **The Aidoku WASM runtime.** WASM invocations are synchronous and CPU-bound. They run on `tokio::task::spawn_blocking` so that a slow or looping source module cannot stall the request path.
- **The job engine.** A pool of workers claims rows from the Postgres `job` table with `FOR UPDATE SKIP LOCKED` and runs handlers concurrently. Each worker is a `tokio::spawn` task, and `download_chapter` fans out page fetches under a per-source `Semaphore`.
- **The SSE fan-out.** Job handlers publish `JobEvent` values to a `tokio::sync::broadcast` channel that every open SSE connection subscribes to.

All three require futures that are `Send` on a work-stealing, multi-threaded Tokio runtime. This requirement is structural. It does not go away as the design evolves.

### The middleware inventory is already Tower-based

The architecture specifies these components by name:

| Concern | Crate |
|---|---|
| Request tracing and span creation | `tower-http::trace::TraceLayer` with a custom `MakeSpan` |
| Session storage in Postgres | `tower-sessions` with `PostgresStore` |
| Rate limiting on `/auth/*`, catalog calls, and telemetry ingest | `tower_governor` |
| W3C Trace Context extraction | `axum-tracing-opentelemetry` |

These are Tower `Layer` implementations. A framework that does not speak Tower requires a replacement for each one.

### SSE is the only live-state channel

`GET /api/v1/events` is the sole mechanism for pushing job progress, state transitions, and new-chapter notifications to the client. The frontend has no polling fallback: a single `EventSource` drives all cache invalidation in TanStack Query. The framework must treat SSE as a supported response type, including keep-alive and access to the `Last-Event-ID` request header.

### Other constraints

- **OpenAPI 3.1 is the frontend contract.** `cargo xtask openapi` writes `web/openapi.json`, which generates the TypeScript client. Drift between routes and the specification breaks the build, so route registration and specification registration should happen in one place.
- **Graceful shutdown is required.** On `SIGTERM` the process stops claiming jobs, waits for in-flight work, and exits. The server future must be drivable to completion under a shutdown signal.
- **Hexagonal boundaries.** The framework is confined to `crates/api`. Nothing in `crates/domain` may depend on it.

## Decision

Use **Axum** for the HTTP API, with these specifics:

- Run on the Tokio multi-threaded runtime under `#[tokio::main]`.
- Pin **`axum = "0.8"`** and treat the 0.9 upgrade as a scheduled, tracked task rather than tracking `main`.
- Compose cross-cutting concerns as Tower layers on the router, not as bespoke wrappers.
- Generate the OpenAPI document with `utoipa` 5.x and bind it to routes through `utoipa-axum`, so that adding a route and registering its schema are the same call.
- Serve SSE with `axum::response::sse::Sse` and the embedded SPA with `rust-embed` behind a fallback route.

> [!IMPORTANT]
> Confine Axum types to `crates/api`. Handlers translate DTOs into domain use-case inputs and map domain errors into problem+json responses. If an `axum::` type appears in a `crates/domain` signature, the dependency rule has been violated.

## Consequences

### What you gain

- **The runtime model matches the workload.** `spawn_blocking` for WASM calls, `tokio::spawn` for job workers, and `broadcast` for SSE all work as documented, with no runtime bridging and no `LocalSet` bookkeeping.
- **Middleware is composition, not integration.** Every component in the inventory above is a Tower `Layer` that applies with `.layer()`. Layers compose at the type level, so the compiler rejects an incorrectly ordered or incompatible stack instead of failing at request time.
- **SSE is a first-class response type.** `Sse` handles event framing and keep-alive, and `Last-Event-ID` is read like any other header, so reconnection and replay need no framework workarounds.
- **The observability design works as written.** Axum is built on `hyper` 1.x and `http` 1.x, the same types `tower-http` and `reqwest` use. `traceparent` extraction, span propagation into job payloads, and the `traceparent` response header are all standard practice in this ecosystem rather than custom code.
- **Maintenance is organizational, not personal.** Axum is maintained in the `tokio-rs` organization alongside the runtime it depends on, which keeps breaking changes aligned with Tokio and Hyper releases.

### What it costs you

- **Axum has not reached 1.0, and 0.9 is in progress.** Breaking changes are already merged on `main`. Plan for the following when you upgrade:

  | Change in 0.9 | Impact here |
  |---|---|
  | `axum::serve` future output type changes: no `io::Result`, and the future is uninhabited without `with_graceful_shutdown` | Touches the `SIGTERM` draining path directly. Review this first. |
  | `axum::serve` applies Hyper's default `header_read_timeout` | May affect long-lived SSE connections and large telemetry uploads. Verify both. |
  | Nested router fallbacks are merged differently | Affects the `rust-embed` SPA fallback and any per-scope 404 behavior. |
  | `#[from_request(via(Extractor))]` surfaces the extractor's rejection type instead of `Response` | Requires error-conversion updates wherever custom extractors are used. |

  > [!NOTE]
  > Budget one focused upgrade task per minor release. Historically Axum has released a breaking minor version roughly once a year (0.6, 0.7, 0.8). This is a recurring cost, not a one-time migration.

- **Fewer batteries in the box.** Multipart handling, static file serving, and TLS termination come from `tower-http`, `axum-extra`, or a reverse proxy rather than from the framework. In this deployment, Caddy terminates TLS and `rust-embed` serves static assets, so the gap is already closed.
- **Trait-bound errors are hard to read.** A handler that fails the `Handler` trait bound produces a long diagnostic that does not name the real problem. Apply `#[axum::debug_handler]` to the handler to get an actionable message.

### Follow-up work this decision creates

1. Pin `axum` and `axum-extra` to explicit `0.8.x` requirements in the workspace `Cargo.toml`, and let Renovate group Axum-ecosystem updates into a single pull request.
2. Add an integration test that asserts the server drains in-flight jobs and exits on `SIGTERM`, so the 0.9 `serve` change fails a test rather than a deployment.
3. Add a CI check that fails when `crates/domain` gains an `axum` dependency.

## Alternatives considered

| Alternative | Latest stable (Sept 2026) | Outcome |
|---|---|---|
| Actix Web | 4.15.0 | Rejected. Runtime model conflicts with the WASM and job-engine design; SSE is not in core. |
| Poem | Actively maintained | Rejected. Smaller ecosystem for the required middleware. |
| Salvo | Actively maintained | Rejected. Same reason as Poem. |
| Rocket | Actively maintained | Rejected. Macro-centric design fits the hexagonal boundary poorly. |
| warp | Maintained, declining adoption | Rejected. Filter-combinator ergonomics degrade on an API this size. |

### Actix Web

Actix Web is the only serious alternative, and it is stronger than Axum on two points that matter:

- **Release stability.** The 4.x line has held source compatibility since February 2022. Over the same period, Axum shipped three breaking minor versions. If upgrade churn were the dominant cost, Actix Web would win on this criterion alone.
- **Throughput.** Public benchmark suites such as TechEmpower place Actix Web in the top five and Axum in the top ten. Treat the numbers as directional: the reported gap is a few percent on plaintext and under roughly two percent on JSON workloads.

It was rejected for three reasons, in order of weight:

1. **Runtime mismatch.** `actix-rt` is a single-threaded runtime replicated once per worker thread, and much of the Actix ecosystem relies on `!Send` futures because that topology guarantees futures never move between threads. This project needs the opposite guarantee. You can run Actix Web under `#[tokio::main]` to get a work-stealing runtime, but then actor support and `actix-web-actors` WebSockets stop working, and the re-exported `spawn()` requires a manually configured `LocalSet` because it calls into the current thread's local set. The result is a framework used against its design intent for the lifetime of the project.
2. **SSE is not in core.** Server-Sent Events support lives in `actix-web-lab`, the maintainers' staging crate for features under evaluation for inclusion. The tracking issue has been open for years. Placing the only live-state channel in the system on an explicitly experimental crate is not an acceptable risk.
3. **A parallel middleware ecosystem.** Every Tower layer in the inventory has an Actix counterpart — `actix-session`, `actix-governor`, `actix-web-opentelemetry` — and all of them are real and maintained. But adopting them means learning a second middleware model and rewriting the observability design against different span hooks, for no compensating benefit.

Throughput did not influence the outcome. This workload is bound by third-party HTTP fetches, WASM execution, and Postgres round-trips, not by framework dispatch overhead.

### A note on OpenAPI tooling

`utoipa` 5.5.0 supports OpenAPI 3.1 and ships integrations for both frameworks (`utoipa-axum` and `utoipa-actix-web`), so tooling was close to neutral. `utoipa-axum` has a small edge: `OpenApiRouter` registers a route and its schema in one call, which makes route-to-specification drift harder to introduce, and `IntoParams` works without an explicit `parameter_in` attribute.

## Revisit this decision when

- Axum 0.9 is released and the upgrade cost exceeds one focused work session.
- The API needs to run as more than one instance, which changes the SSE fan-out design and may make the framework choice secondary to a shared message bus.
- Framework dispatch overhead appears in a profile as a measurable share of request latency.

## References

- [Axum repository and releases](https://github.com/tokio-rs/axum/releases)
- [Axum changelog, including unreleased 0.9 changes](https://github.com/tokio-rs/axum/blob/main/axum/CHANGELOG.md)
- [Announcing Axum 0.8.0](https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0)
- [`actix-rt` documentation on the single-threaded runtime model](https://docs.rs/actix-rt/latest/actix_rt/)
- [`actix_web::main` compared with `tokio::main`](https://github.com/actix/actix-web/discussions/2852)
- [`actix-web-lab::sse`](https://docs.rs/actix-web-lab/latest/actix_web_lab/sse/index.html)
- [Actix Web issue 186: EventSource / Server-Sent Events support](https://github.com/actix/actix-web/issues/186)
- [utoipa documentation](https://docs.rs/utoipa/latest/utoipa/)
- [tower-sessions](https://github.com/maxcountryman/tower-sessions)
