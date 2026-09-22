# ADR-0013: Ingest frontend telemetry through the API

| | |
|---|---|
| **Status** | Accepted, with a phased frontend scope |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/api` (`POST /api/v1/telemetry`), `web/src/shared/lib/telemetry.ts` |
| **Supersedes** | None |

This article explains why browser telemetry is sent to an endpoint on the API rather than to a collector, and why the frontend starts with `traceparent` propagation plus error reporting instead of the full OpenTelemetry web SDK. The endpoint decision is straightforward; the frontend scope is a deliberate reduction from the architecture draft, and the reason is bundle cost.

## Context

The frontend architecture draft opens by naming an inconsistency: the backend has no OTLP receiver, yet the frontend was specified to use the OpenTelemetry web SDK. This ADR resolves it.

### What the frontend actually needs telemetry for

Two things, in priority order:

1. **Correlation.** When something fails, the crash page should show a trace ID that appears in the backend logs, so `grep <trace_id>` returns the request, the job it enqueued, and the WASM call that failed. This is the debugging workflow [ADR-0014](0014-keep-backend-telemetry-as-structured-logs.md) is built around.
2. **Client-side error visibility.** An exception in the SPA currently goes nowhere. The operator has no way to know a route crashed.

Page timings and user-interaction spans are a third, weaker want: useful, but not what makes a failure diagnosable.

### The OTel web SDK is expensive, and its instrumentations are pre-1.0

| Package | Version | Unpacked |
|---|---|---|
| `@opentelemetry/sdk-trace-web` | 2.11.0 | 227 KB |
| `@opentelemetry/instrumentation-fetch` | **0.222.0** | 253 KB |
| `@opentelemetry/instrumentation-document-load` | **0.67.0** | 147 KB |
| `@opentelemetry/instrumentation-user-interaction` | **0.66.0** | 215 KB |
| `@opentelemetry/exporter-trace-otlp-http` | **0.222.0** | 58 KB |

Assembled carelessly this is 200 KB or more of JavaScript shipped before a user can interact with the application, which pushes blocking time and can move LCP past the 2.5 s threshold. It can be brought to roughly **30 KB gzipped** — by dropping `zone.js` for `StackContextManager`, enabling ESM tree-shaking, importing selectively, and lazy-loading — but that is work, and it has to be maintained against packages still on `0.x`. The Browser SIG itself acknowledges the SDK was not built browser-first.

> [!WARNING]
> Do not use `@opentelemetry/context-zone`. `zone.js` is roughly a megabyte on its own. Use `StackContextManager`.

### Correlation does not require the SDK

A W3C `traceparent` header is a version, a 16-byte trace ID, an 8-byte span ID, and a flag byte. Generating one and attaching it in the `openapi-fetch` middleware from [ADR-0009](0009-hand-write-query-hooks-with-semantic-key-factories.md) is a small amount of code and no dependency. The backend already continues that trace and returns `traceparent` on the response.

So the expensive part of the draft buys the *third* priority, not the first two.

### The backend has no collector, by decision

ADR-0014 rejected a collector on resource grounds: a single-node LGTM deployment starts at 4 vCPU and 8 GB of RAM, on a host already running PostgreSQL and FlareSolverr.

## Decision

### The endpoint

Accept browser telemetry at **`POST /api/v1/telemetry`**, and emit each span as a `tracing` event into the same JSON log stream as everything else.

- **Wire format is OTLP.** Accept `application/json` (OTLP/JSON `ExportTraceServiceRequest`) and `application/x-protobuf`, decoded with `opentelemetry-proto` using `gen-tonic-messages` and `with-serde`. Choosing OTLP even for hand-rolled payloads is what keeps the migration path open.
- **Authentication is the session cookie**, per [ADR-0005](0005-act-as-the-oidc-client-with-server-side-sessions.md). No separate credential.
- **Limits:** max body 256 KB, max 200 spans per batch, per-session rate limit around 30 requests per minute via `tower_governor`, plus a per-IP limit. `TELEMETRY_INGEST_ENABLED` is a kill switch.
- **Validation is an allow-list, not a deny-list.** Reject any batch whose resource `service.name` is not `manga-web`. Keep only attributes matching `http.*`, `url.*`, `exception.*`, `browser.*`, and `app.*`. Truncate strings to 1 KB. Cap attribute counts.
- **Identity comes from the server.** `session.id` is the hashed server-side session, never a client-supplied field. The client cannot set `service.name`, `session.id`, or `user.id`.
- **Responses:** `202 Accepted` with an empty OTLP body; `400` problem+json for validation failures; `429` with `Retry-After` when throttled. Never `5xx` for bad client input.
- **Under `--features otlp`** the endpoint forwards the batch to the configured collector instead of logging it, so the frontend never changes.

### The frontend, in two phases

This is where the ADR departs from the draft.

**Phase 1, shipped in v1 — no OTel web SDK:**

- Generate a `traceparent` per user interaction in `shared/lib/telemetry.ts` and attach it in the `openapi-fetch` middleware.
- Read the `traceparent` response header and keep the current trace ID available to the root error boundary, so the crash page can display it.
- On an uncaught exception or an error boundary trigger, `POST` a hand-built OTLP/JSON batch containing one span with `exception.type`, `exception.message`, `exception.stacktrace`, `url.path`, and the app version. Errors are reported at **100%**.
- Bundle cost: a few hundred bytes.

**Phase 2, when page timings are actually wanted:**

- Add `@opentelemetry/sdk-trace-web` with `StackContextManager`, plus `document-load` and `fetch` instrumentations, **lazy-loaded** and never in the main chunk, gated behind `VITE_OTEL`.
- Sample traces at 10%, keep errors at 100%.
- Add a `size-limit` budget for the telemetry chunk specifically, separate from the main chunk.

> [!IMPORTANT]
> Phase 1 delivers both priorities from the context section. Phase 2 delivers the third. Shipping Phase 2 first would put 200 KB in front of every page load for data nobody has yet asked a question of.

## Consequences

### What you gain

- **The inconsistency is resolved** without adding a container, and the frontend and backend share one log stream keyed by `trace_id`.
- **One authenticated, rate-limited, validated ingress**, rather than a collector endpoint that has to be exposed to browsers and given its own authentication.
- **Full correlation on day one at negligible bundle cost.** The crash page shows a trace ID that `grep` resolves into the whole causal chain.
- **Errors are never sampled.** A 10% trace sample would lose most crashes; reporting exceptions at 100% separately avoids that.
- **The migration path stays open.** OTLP on the wire plus the `otlp` feature means a later collector requires no frontend change.
- **Phase 2 is additive**, and its cost is isolated behind a lazy chunk with its own budget.

### What it costs you

- **The endpoint writes user-controlled data into the log stream.** This is a log-injection surface: a browser can send arbitrary strings that land in JSON lines an operator later reads and pipes through `jq`. The allow-list, the truncation, the count caps, and correct JSON escaping are the controls, and they must be tested rather than assumed.
- **It can blow out the retention window.** ADR-0014 bounds history with Docker `json-file` rotation at `50m × 10`. A session pushing 30 requests per minute at 200 spans each is 6,000 spans per minute of log volume from one browser tab. Left unbudgeted, frontend telemetry could evict the backend history that made the design worth having.

  > [!IMPORTANT]
  > Measure the interaction between ingest limits and the rotation policy before enabling Phase 2. If web telemetry is a meaningful share of log volume, give `web_telemetry` a separate log target rather than letting it compete with backend history.

- **Phase 1 is code you own.** `traceparent` generation, sampling flags, and a minimal OTLP/JSON payload builder are small but hand-written, and getting the trace-ID format wrong silently breaks correlation.
- **Phase 2 depends on `0.x` instrumentation packages** whose versions move independently of the 2.x SDK core.
- **No session replay, no breadcrumbs, no grouping.** This is error *reporting*, not an error-tracking product. Two occurrences of the same crash are two log lines, not one issue with a count.
- **Client clocks are unreliable.** Browser-reported timings and timestamps can be skewed or deliberately wrong; treat client-supplied time as advisory and stamp server receipt time.

### Follow-up work this decision creates

1. **Test `traceparent` round-tripping end to end**: a span generated in the browser, propagated on a fetch, continued by the Axum server span, stored in a job payload, and appearing on the worker's log lines with the same `trace_id`.
2. **Fuzz or property-test the OTLP decode path.** It parses attacker-influenced bytes from an authenticated but untrusted client. Assert that oversized batches, deep nesting, and hostile strings are rejected without a `5xx`.
3. **Test the allow-list and the identity override**: a client claiming `service.name: manga-api`, or supplying its own `session.id` or `user.id`, must be rejected or have those fields replaced.
4. **Test log-injection resistance**: a span name containing newlines, quotes, and ANSI escapes must serialize into one valid JSON line that `jq` parses.
5. **Measure log volume from telemetry** against the rotation policy, and decide on a separate target before Phase 2.
6. **Verify the error path works when the app is broken.** The reporter must not depend on the router, the Query client, or the i18n catalog, since all three may be why the page crashed.
7. **Add the `size-limit` budget for the telemetry chunk** at the start of Phase 2, not after.

## Alternatives considered

| Alternative | Added containers | Browser-reachable surface | Frontend bundle | Outcome |
|---|---|---|---|---|
| Ingest through the API | 0 | The API, already exposed | ~0 (Phase 1) | **Chosen** |
| Browser → OTel collector | 1+ | A collector needing its own auth | Same SDK cost | Rejected. Adds the component ADR-0014 avoided. |
| No frontend telemetry | 0 | None | 0 | Rejected. Loses crash visibility and the trace ID. |
| Sentry or similar SaaS | 0 | Third-party | ~25 KB gzipped | Rejected on data egress; best DX. |
| Self-hosted GlitchTip or Sentry | 2+ | Another service | ~25 KB gzipped | Rejected on resources. |
| Full OTel web SDK in v1 | 0 | The API | 200 KB+, or 30 KB with work | Rejected as Phase 1; deferred to Phase 2. |

### Browser to an OpenTelemetry collector

The clean long-term architecture: both frontend and backend export OTLP to a collector that fans out to a trace store. Rejected for the reason given in ADR-0014 — it adds containers and RAM to a constrained host — plus one specific to the frontend: **a collector that browsers talk to must be reachable from the public internet and must authenticate them.** The session cookie is the only credential the browser has, and a collector does not understand it. Making that work means putting the API in front of the collector, which is this decision with an extra hop.

### No frontend telemetry at all

Genuinely tempting for a single-user self-hosted application, and it was considered seriously. Rejected because it loses both priorities: no trace ID on the crash page, and no signal at all when a route throws. Phase 1 costs a few hundred bytes to recover both, which is a better trade than zero.

### An error-tracking service

Sentry and its peers give grouping, release tracking, source-mapped stack traces, and breadcrumbs — a materially better debugging experience than log lines, for roughly 25 KB gzipped. Rejected for the same reason as hosted telemetry in ADR-0014: this application's telemetry describes a user's reading habits, and shipping that to a third party is not a default a self-hosted media application should choose for its operator. Self-hosting the same tooling means two or more additional containers on a host that cannot afford them.

### The full OTel web SDK in v1

This is the draft's position and it is not wrong in the long run — automatic `document-load`, `fetch`, and `user-interaction` spans are real data that hand-rolled reporting will not produce. It was rejected *for v1* on sequencing: the bundle cost is paid by every page load from the first release, while the data it produces has no question waiting for it. Phase 2 exists precisely so this arrives when someone wants to ask that question, with the mitigations known in advance rather than discovered under a performance regression.

## Revisit this decision when

- **Page-load or interaction latency becomes a complaint.** That is the trigger for Phase 2, and the point at which document-load and fetch instrumentation earns its size.
- **Frontend telemetry becomes a significant share of log volume.** Split it to its own target before it evicts backend history.
- **A collector is introduced for other reasons.** The `otlp` feature makes the endpoint forward instead of log, and no frontend change is needed.
- **Crash triage needs grouping and release tracking.** That is an error-tracking product, and the honest options are self-hosting one or accepting data egress — a decision for the operator, not the project.
- **The OTel browser SDK ships a browser-first build** with the instrumentations at 1.0. That would substantially weaken the sequencing argument for Phase 1.

## References

- [W3C Trace Context](https://www.w3.org/TR/trace-context/) — the `traceparent` format Phase 1 implements
- [`@opentelemetry/sdk-trace-web`](https://www.npmjs.com/package/@opentelemetry/sdk-trace-web) — 2.11.0
- [`@opentelemetry/instrumentation-fetch`](https://www.npmjs.com/package/@opentelemetry/instrumentation-fetch) — 0.222.0, pre-1.0
- [Reducing OpenTelemetry bundle size in the browser](https://signoz.io/blog/reduce-opentelemetry-bundle-size-for-browser-frontend/) — directional; the ~30 KB gzipped target and `StackContextManager`
- [opentelemetry-js issue 4817: bundle size too large for js-web](https://github.com/open-telemetry/opentelemetry-js/issues/4817)
- [OpenTelemetry experts on the future of browser support](https://embrace.io/blog/opentelemetry-experts-share-the-future-of-browser-support/) — directional; "not built with a browser-first mentality"
- [`opentelemetry-proto`](https://crates.io/crates/opentelemetry-proto) — 0.33.0, OTLP/JSON decoding on the server
- [OTLP specification](https://opentelemetry.io/docs/specs/otlp/)
