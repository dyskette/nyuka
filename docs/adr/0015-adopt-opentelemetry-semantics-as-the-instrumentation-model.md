# ADR-0015: Adopt OpenTelemetry semantics as the instrumentation model

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | All crates; `web/src/shared/lib/telemetry.ts` |
| **Supersedes** | None |
| **Related** | [ADR-0013](0013-ingest-frontend-telemetry-through-the-api.md), [ADR-0014](0014-keep-backend-telemetry-as-structured-logs.md) |

This article explains why instrumentation uses OpenTelemetry's data model, semantic conventions, and W3C Trace Context propagation — independently of whether anything is ever exported to an OpenTelemetry backend. It is the foundation ADR-0013 and ADR-0014 build on, and it corrects three attribute-naming decisions in the architecture draft that no longer match the stable conventions.

> [!NOTE]
> This ADR is numbered after the two it underpins, because ADRs are numbered in the order decisions are recorded rather than in dependency order. Read this one first when onboarding.

## Context

### Three separate questions were tangled together

The architecture draft lists "Observability | OpenTelemetry" as a locked decision, then specifies a log-only pipeline. Those are not in conflict, but they are three decisions:

| Question | Decided in |
|---|---|
| What vocabulary and data model does instrumentation use? | **This ADR** |
| Where does the telemetry go? | ADR-0014 — structured logs, no collector |
| How does browser telemetry reach the backend? | ADR-0013 — an API endpoint |

Separating them matters because the answer to the first is "OpenTelemetry" even though the answer to the second is "not an OpenTelemetry backend". Adopting the *semantics* without adopting the *pipeline* is the whole shape of this design, and it only works if the semantics are applied deliberately.

### Most Rust libraries instrument with `tracing`, not the OTel API

This is the practical constraint. `axum`, `tower-http`, `sqlx`, `reqwest`, and `hyper` all emit `tracing` spans and events. Instrumenting the application against the OpenTelemetry API directly would produce two disjoint span trees — the libraries' and the application's — with no parent-child relationship between them. The OpenTelemetry Rust project's own guidance is to keep using `tracing` and bridge it, and it recommends `tracing` for new Rust applications.

### Conventions have stabilized, and the draft predates that

OpenTelemetry semantic conventions now carry explicit stability levels — **Development** (formerly Experimental, renameable without deprecation), **Release Candidate**, and **Stable** (locked, only deprecated in favor of a successor). HTTP and database conventions both have stable forms, with published migration guides.

The Rust crate reflects this split directly: `opentelemetry-semantic-conventions` exposes only stable conventions by default, with a `semconv_experimental` feature for the rest.

The draft's attribute names were written before the database conventions stabilized, and three of its choices are now wrong. They are corrected below.

### Correlation has to survive three process boundaries

A single causal chain crosses the browser, the HTTP request, and a job executed later by a worker — possibly minutes later, for a deferred download. Nothing proprietary spans those boundaries. W3C Trace Context does: `traceparent` propagates browser to server, and the trace context stored in `job.payload.trace_context` propagates request to worker.

## Decision

Instrument with `tracing`, bridge it to OpenTelemetry, and use OpenTelemetry semantic conventions and W3C Trace Context as the vocabulary and the wire format.

- **`tracing` is the instrumentation API.** Application spans come from `#[tracing::instrument]` on domain use cases and repository methods, not from the OpenTelemetry API. Library spans are inherited.
- **`tracing-opentelemetry` is the bridge**, giving every span a real OTel span and trace ID and honoring propagated context. Per the pin in ADR-0014, hold all OpenTelemetry crates on the **0.32** line.
- **Use stable semantic conventions only.** Do not enable `semconv_experimental`. Where no stable convention exists, use the custom namespace rule below.
- **W3C Trace Context is the propagation format**, both inbound (`traceparent`, `tracestate`) and outbound.

### Attribute naming: three corrections to the draft

#### 1. Database attributes were renamed

The stable database conventions replaced the names the draft uses:

| Draft | Stable name | Level |
|---|---|---|
| `db.system` | **`db.system.name`** | Required |
| `db.operation` | **`db.operation.name`** | Conditionally required |
| `db.sql.table` | **`db.collection.name`** | Conditionally required |
| `db.statement` | **`db.query.text`** | Recommended |

Also available and worth adopting: `db.namespace`, `db.response.status_code`, `server.address`, `server.port`, and **`db.query.summary`** — a low-cardinality query summary that is the preferred database span name.

Database span names follow a defined priority: `{db.query.summary}` if available, otherwise `{db.operation.name} {target}`, otherwise `{target}`, otherwise `{db.system.name}`.

#### 2. The route template belongs in `http.route`, not `url.path`

The draft says to put the route template in `url.path` "to avoid cardinality". That conflates two stable attributes:

- **`http.route`** is the low-cardinality route template — `/api/v1/manga/{id}`.
- **`url.path`** is the actual request path — `/api/v1/manga/42`.

Record the template in `http.route` and use it in the span name. Omit `url.path`, or record it knowing it is high-cardinality. The draft's other HTTP attributes — `http.request.method`, `http.response.status_code`, `client.address` — are correct as written.

#### 3. Hand-rolled `traceparent` must satisfy Trace Context Level 2

Trace Context **Level 1 is a W3C Recommendation**; **Level 2 is a Candidate Recommendation Draft**, and OpenTelemetry is adopting Level 2 as the basis for consistent sampling. Level 2 adds a **random-trace-id flag** and requires the least-significant 56 bits of the trace ID to be sufficiently random.

> [!IMPORTANT]
> ADR-0013 Phase 1 generates `traceparent` by hand rather than using the OTel web SDK. That generator must set the random flag and produce trace IDs whose low 56 bits are genuinely random — use `crypto.getRandomValues`, never `Math.random`. Getting this wrong produces IDs that correlate correctly today and silently break consistent sampling if an exporter is ever enabled. Cover it with a test asserting format and entropy.

### Custom attributes go in one namespaced prefix

The design needs attributes OpenTelemetry has no stable convention for: job execution, WASM invocations, source identity, SSE connections, and FlareSolverr detours.

Put all of them under a single project prefix — `nyuka.*` — rather than in bare top-level namespaces:

| Draft | Use instead |
|---|---|
| `job.id`, `job.kind`, `job.attempt` | `nyuka.job.id`, `nyuka.job.kind`, `nyuka.job.attempt` |
| `source.id`, `source.version` | `nyuka.source.id`, `nyuka.source.version` |
| `aidoku.fn`, `wasm.fuel_used`, `wasm.memory_peak_bytes` | `nyuka.aidoku.fn`, `nyuka.wasm.*` |
| `cf.challenge` | `nyuka.cf.challenge` |
| `sse.events_sent` | `nyuka.sse.events_sent` |

The reason is collision avoidance: a bare `job.*` or `wasm.*` namespace may later be standardized by OpenTelemetry with different semantics, at which point existing data means two things. Span *names* such as `job.run` and `aidoku.call` can stay as they are — the constraint is on attribute keys.

For frontend telemetry, ADR-0013's allow-list already permits `app.*`; keep browser-side custom attributes there.

### Resource attributes

`service.name`, `service.version` (from `CARGO_PKG_VERSION` plus git SHA via `vergen`), and `deployment.environment.name` — note the stable name includes `.name`. The frontend reports `service.name=manga-web`, which ADR-0013's ingest validation enforces server-side.

## Consequences

### What you gain

- **One span tree.** Application spans nest inside `tower-http`'s server span and contain `sqlx` and `reqwest` spans, because everything is `tracing` underneath.
- **Correlation across three boundaries** using a W3C Recommendation rather than anything invented here.
- **A vocabulary that means something to other tools.** `db.system.name` and `http.route` are understood by every OpenTelemetry-aware backend, so log lines are interpretable by something other than this project's own `jq` recipes.
- **The migration in ADR-0014 is genuinely a flag.** Because attributes already follow stable conventions, enabling the exporter produces correct data rather than data needing a translation layer.
- **Stable-only means no silent renames.** Declining `semconv_experimental` is what makes attribute names a stable contract instead of a moving target.
- **Namespacing protects the custom attributes** from a future convention landing on the same keys.

### What it costs you

- **Conventions are verbose and easy to get subtly wrong**, as the three corrections above demonstrate — and those were in a carefully written draft. Attribute naming needs review attention, because a wrong name is not a compile error and not a test failure.
- **You inherit OpenTelemetry's version churn** even without exporting. ADR-0014 documents the pin; this ADR is why the dependency exists at all.
- **Stable-only leaves gaps.** There is no stable convention for background job execution or WASM, which is why the custom prefix exists. If OpenTelemetry later standardizes those areas, migrating is real work — bounded, because everything custom shares one prefix.
- **`tracing` and OpenTelemetry are not a perfect fit.** `tracing`'s events are not OTel log records, and `tracing-opentelemetry` bridges traces and metrics but **not logs**; log export would need `opentelemetry-appender-tracing`. This is invisible while logs go to stdout, and becomes a decision if that changes.
- **Semantic conventions are a specification to read.** The team carries a small ongoing obligation to check names against the spec rather than inventing them.

### Follow-up work this decision creates

1. **Correct the attribute names in the architecture document** — the three items above — so the stale names do not reach the code.
2. **Define the attribute vocabulary in one module.** Constants for every attribute key, stable ones re-exported from `opentelemetry-semantic-conventions` and custom ones declared under the `nyuka.` prefix. String literals at call sites are how naming drifts.
3. **Add `db.query.summary`** to repository instrumentation and use it as the database span name, per the naming priority.
4. **Test the `traceparent` generator** in the frontend for format compliance, the random flag, and 56-bit entropy from a cryptographic source.
5. **Add a test asserting no attribute key sits outside** the stable conventions or the `nyuka.` and `app.` prefixes, so the namespacing rule is enforced rather than remembered.
6. **Confirm `semconv_experimental` is not enabled** anywhere in the workspace, including transitively.

## Alternatives considered

| Alternative | Cross-process correlation | Vocabulary | Outcome |
|---|---|---|---|
| `tracing` bridged to OpenTelemetry semantics | W3C Trace Context | Stable semconv | **Chosen** |
| `tracing` with ad-hoc field names | Hand-rolled | Invented | Rejected. Cheaper now, unportable later. |
| OpenTelemetry API directly | W3C Trace Context | Stable semconv | Rejected. Splits the span tree from library spans. |
| `log` with structured output | None | Invented | Rejected. No spans, no causality. |
| A vendor SDK | Vendor format | Vendor | Rejected. Couples instrumentation to a backend. |

### `tracing` with ad-hoc field names

The minimal option: keep `tracing`, drop the OpenTelemetry dependency entirely, and name fields whatever is convenient. No version pin, no `opentelemetry` crates, no convention spec to read.

Rejected because it gives up the two things that make the log-only design in ADR-0014 tolerable. First, there would be **no trace or span IDs** unless the project generated and threaded them itself — which is reimplementing the part of the SDK actually in use. Second, the data would be **portable nowhere**: the "migration is a feature flag" property depends on attributes already being conventional, and without it, adopting a collector later means rewriting every instrumentation site rather than setting an environment variable.

It is the honest cheap option, and it is cheap only until the first time the telemetry needs to leave the box.

### The OpenTelemetry API directly

Instrumenting against `opentelemetry::trace` rather than `tracing` would be more idiomatic for an OpenTelemetry-native application and would remove the bridge crate — along with its version-lag problem.

Rejected because of what the ecosystem does. `axum`, `tower-http`, `sqlx`, `reqwest`, and `hyper` emit `tracing` spans. Instrumenting elsewhere yields two unrelated span trees, so a slow query would not appear under the request that issued it. The OpenTelemetry Rust project recommends `tracing` for new applications for exactly this reason.

### `log` with structured output

Rejected on capability. `log` has records, not spans, so there is no parent-child structure and no duration. The central need here is following one causal chain across an HTTP request into a job that runs later, which flat log lines cannot express regardless of how well they are structured.

### A vendor SDK

Datadog, Sentry, or a similar Rust SDK would give a better out-of-box experience with its own backend. Rejected because it couples instrumentation to a vendor, and because the data-egress objection recorded in ADR-0014 and ADR-0013 applies: this application's spans describe a user's reading habits, and that is the operator's decision to make, not the project's.

## Revisit this decision when

- **OpenTelemetry publishes stable conventions for background jobs or WASM.** Migrate the `nyuka.`-prefixed attributes in those areas; the prefix is what makes that a bounded change.
- **Trace Context Level 2 reaches Recommendation.** Re-check the hand-rolled generator against the final text, particularly the flag semantics.
- **Log export becomes desirable.** `tracing-opentelemetry` does not bridge logs, so that means adding `opentelemetry-appender-tracing` and deciding whether `tracing` events should become OTel log records.
- **The OpenTelemetry Rust traces API and SDK reach stable.** That is also ADR-0014's trigger, and the two should be reconsidered together.
- **Attribute naming errors reach production more than once.** That is evidence the constants module and the namespacing test are not doing their job, and the rule needs stronger enforcement.

## References

- [OpenTelemetry semantic conventions](https://opentelemetry.io/docs/specs/semconv/) — stability levels
- [Semantic conventions for database client spans](https://opentelemetry.io/docs/specs/semconv/db/database-spans/) — `db.system.name`, `db.operation.name`, `db.collection.name`, `db.query.summary`, span naming priority
- [Database semantic convention stability migration guide](https://opentelemetry.io/docs/specs/semconv/non-normative/db-migration/)
- [Semantic conventions for HTTP](https://opentelemetry.io/docs/specs/semconv/http/) — `http.route` versus `url.path`
- [W3C Trace Context (Level 1, Recommendation)](https://www.w3.org/TR/trace-context/)
- [W3C Trace Context Level 2 (Candidate Recommendation Draft)](https://www.w3.org/TR/trace-context-2/)
- [OpenTelemetry sampling milestones](https://opentelemetry.io/blog/2025/sampling-milestones/) — Level 2 adoption and the random flag
- [OpenTelemetry Rust](https://opentelemetry.io/docs/languages/rust/) — guidance to instrument with `tracing`
- [`opentelemetry-semantic-conventions`](https://crates.io/crates/opentelemetry-semantic-conventions) — stable by default, `semconv_experimental` opt-in
- [`tracing-opentelemetry`](https://crates.io/crates/tracing-opentelemetry) — bridges traces and metrics, not logs
