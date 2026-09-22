# ADR-0014: Keep backend telemetry as structured logs, not a collector pipeline

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/api` (subscriber setup), all crates (instrumentation) |
| **Supersedes** | None |

This article explains why backend telemetry is `tracing` emitting JSON to stdout, with no OTLP exporter and no collector, while still producing W3C-correlatable trace IDs. It also records the version pin this requires and the maturity fact that makes exporting traces from Rust today a worse bet than it sounds.

## Context

### The debugging question is causal, not statistical

The thing an operator needs to answer is: *a user clicked download and it failed — what happened?* That chain crosses the browser, an HTTP request, a job enqueued with a future `run_at`, a worker execution, a WASM source call, and possibly a FlareSolverr detour. Answering it requires following one causal thread, not querying aggregates.

That is why the observability design centers on a `trace_id` carried end to end: the SPA propagates `traceparent` into `/api/v1/*`, the Axum server span continues that trace, the job enqueue stores the context in `job.payload.trace_context`, and the worker's `job.run` span links back to it. With that in place, `grep <trace_id>` reconstructs the whole story.

### RAM on the VM is the scarce resource

The deployment is one Compose host already running `api`, `postgres:17`, `flaresolverr`, and optionally `caddy`. FlareSolverr drives a real browser. Postgres wants page cache. Anything added competes with the two services that do the work.

### The design already specifies the log contract

The observability section fixes most of the details independently of this decision: JSON lines carrying `trace_id` and `span_id`, a span model with attributes for HTTP, database, jobs, and WASM calls, resource attributes (`service.name`, `service.version` from `CARGO_PKG_VERSION` plus git SHA, `deployment.environment`), a field-redaction layer masking `authorization|cookie|set-cookie|token|secret|password`, log-derived metric snapshots every 60 seconds, and Docker `json-file` rotation at `max-size=50m`, `max-file=10`.

### Traces are the least mature part of the Rust OpenTelemetry SDK

This is the fact that most changes the calculation, and it runs counter to intuition:

| Component | Status on the 0.32/0.33 line |
|---|---|
| Logs API and SDK | **Stable** |
| Metrics API and SDK | **Stable** |
| **Traces API and SDK** | **Beta** |
| OTLP exporter, logs and metrics | Release candidate |
| **OTLP exporter, traces** | **Beta** |

Rust's OpenTelemetry implementation stabilized logs and metrics *before* traces, which is the opposite of most languages. A design whose value comes from trace correlation would, by exporting, take a dependency on the beta parts.

## Decision

Emit telemetry as `tracing` events serialized to JSON on stdout. Do not run a collector, and do not export OTLP in v1.

- **Keep the `OpenTelemetryLayer` without an exporter.** This is the key move: the layer creates real span and trace IDs and honors propagated context, so every JSON line is joinable by `trace_id` and distributed correlation works — without an export path. The subscriber stack stays as designed: `EnvFilter` → `ErrorLayer` → `OpenTelemetryLayer` → `fmt::layer().json()`.
- **Pin `opentelemetry = "0.32"`, not 0.33.**

  > [!IMPORTANT]
  > `opentelemetry` and `opentelemetry_sdk` are published at 0.33.0, but `tracing-opentelemetry` 0.33.0 requires `opentelemetry ^0.32`, and `axum-tracing-opentelemetry` 0.39.1 requires `opentelemetry ^0.32` and `opentelemetry-semantic-conventions ^0.32`. The bridge crates lag the SDK by a minor version. Pin the 0.32 line across all OTel crates and upgrade them as one group, or the build will not resolve.

- **Feature-gate the exporter.** `opentelemetry-otlp` sits behind `--features otlp`, activated at runtime by `OTEL_EXPORTER_OTLP_ENDPOINT`. Sampling variables (`OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG`) affect export only. **Logs are never sampled.**
- **Metrics are log-derived.** A `metrics` module emits `tracing::info!(target: "metrics", …)` snapshots every 60 seconds with `jobs.queued`, `jobs.running`, `jobs.failed_1h`, `jobs.p95_duration_ms` by kind, `downloads.bytes_total`, `sse.connections`, `wasm.instances_active`, `db.pool.in_use`, and `disk.library_free_bytes`. No `/metrics` endpoint.
- **Health is separate from telemetry.** `/healthz` reports the process is up; `/readyz` checks the database, migration currency, library writability, and FlareSolverr reachability or a degraded flag.
- **The day-one workflow is `docker logs manga-api | jq`.** Treat that as a supported interface: if a field is hard to filter on, that is a bug in the log schema.

## Consequences

### What you gain

- **Trace correlation without an exporter.** `traceparent` extraction, context propagation into job payloads, span links from `job.run` back to the enqueuing request, and the `traceparent` response header that lets the SPA show a trace ID on its crash page — all work. The only thing missing is a place to *visualize* them.
- **No added containers, no added RAM.** The observability stack costs a serializer.
- **Logs are complete.** Because sampling only affects export, there is no sampled-away line when investigating a failure. For a low-traffic single-tenant service, full fidelity beats aggregate queryability.
- **Minimal exposure to beta APIs.** The SDK surface in use is span creation and context propagation. The beta trace exporter is not on the path.
- **The migration is a feature flag, not a refactor.** Instrumentation is already OTel-semantic. Turning on `otlp` and setting an endpoint routes the same spans to a collector, and per [ADR-0013](0013-ingest-frontend-telemetry-through-the-api.md) the frontend ingest endpoint forwards instead of logging under the same flag.
- **Secrets stay out.** One redaction layer covers every sink because there is only one sink.

### What it costs you

- **No dashboards, no alerting, no trace waterfall.** Causality is reconstructed by reading `jq` output, which works well for one request and poorly for "what changed this week".
- **Aggregates are computed in-process and only sampled every 60 seconds.** `jobs.p95_duration_ms` is a number in a log line, not a queryable series. Comparing this week to last week means parsing rotated log files.
- **Retention is a rotation policy.** `50m × 10` bounds history to whatever that holds, which under a busy download period could be days rather than weeks. Anything older is gone, including the evidence for a slow regression.
- **SLO measurement is weak.** The architecture defers two SLOs — job success rate and time-to-first-page — to before production. Log-derived snapshots can *report* them but cannot easily *evaluate* them over a window, which is what an SLO needs. This is the sharpest limitation of the decision.
- **`grep` assumes one log stream.** Single-instance operation, consistent with ADR-0003 and ADR-0010, is load-bearing here too.
- **OTel version churn still lands on you.** Even without an exporter, the pin above must be maintained as a group, and bridge crates trailing the SDK is an ongoing condition rather than a one-off.

### Follow-up work this decision creates

1. **Define the two deferred SLOs concretely**, and decide how each is measured given log-only telemetry. If either needs a window-based evaluation, that is evidence for the `vector`/Loki step sooner rather than later.
2. **Pin all OTel crates to 0.32 in one Renovate group**, with `tracing-opentelemetry` and `axum-tracing-opentelemetry` in the same group so an upgrade is one reviewed change.
3. **Test the redaction layer** with an authorization header, a cookie, and a token-bearing query string, asserting none appear in serialized output. This is a security control, so it needs a test rather than a code review.
4. **Assert `trace_id` is present on every line** in a test that exercises a request end to end, including the job log lines it causes. Silent loss of correlation is the failure that makes this whole design worthless.
5. **Document the `jq` recipes** in the runbook: follow one trace, list failed jobs in the last hour, find slow WASM calls, extract the latest metrics snapshot.
6. **Measure log volume during a large download** and size the rotation policy against it, so retention is a known number rather than an assumption.

## Alternatives considered

| Alternative | Added containers | Added RAM | Queryable history | Outcome |
|---|---|---|---|---|
| `tracing` → JSON stdout | 0 | ~0 | Rotation window, via `jq` | **Chosen** |
| Full OTLP → collector → LGTM | 4–5 | 8 GB minimum | Yes, with dashboards | Rejected. Cost exceeds the whole application. |
| OTLP → hosted backend | 0–1 | Low | Yes | Rejected on data egress; closest alternative. |
| `/metrics` endpoint + Prometheus | 1–2 | Moderate | Metrics only | Rejected. Answers the wrong question. |
| `vector`/`alloy` sidecar, logs → Loki | 1–2 | Moderate | Yes | Deferred, not rejected. The documented next step. |

### A full OTLP pipeline with the LGTM stack

The collector plus Tempo, Loki, Prometheus or Mimir, and Grafana is what this design is a substitute for, and it is genuinely better at everything except cost: trace waterfalls, log-to-trace linking, real dashboards, alerting, and retention measured in weeks.

Rejected on resource cost, with numbers. Published guidance for a single-node LGTM deployment puts the floor at **4 vCPU and 8 GB of RAM for light workloads**, with 16 vCPU and 32 GB for comfortable operation. That floor is larger than the rest of this deployment combined, on a host that also runs PostgreSQL and a browser-driving challenge solver. The OpenTelemetry Collector additionally ships a **50 MiB default memory limiter that drops data or crash-loops under sustained bursts** until raised — meaning the observability stack acquires its own failure modes and its own tuning burden.

There is a second problem specific to this application: the frontend also produces spans, so a collector would have to be reachable from browsers, which means authenticating it. ADR-0013 solves that by ingesting through the API, and that solution is what makes the collector optional rather than necessary.

Deferred rather than dismissed: the `otlp` feature and the ingest endpoint exist so this becomes a configuration change.

### OTLP to a hosted backend

Pointing the exporter at Grafana Cloud, Honeycomb, or a similar service gets dashboards and retention with no local RAM cost, and it is the option a different operator might reasonably prefer. It is the closest alternative.

Rejected for two reasons. First, **data egress**: this application's spans carry source names, manga titles, and URL paths, which together describe a user's reading habits. Shipping that to a third party is a privacy decision a self-hosted media application should not make on its operator's behalf, and defaulting to it would be wrong even if the operator could turn it off. Second, exporting traces means depending on the **beta** trace exporter for the primary value of the system.

An operator who wants this can have it: build with `--features otlp` and set the endpoint. That is deliberate — it is a choice the deployer makes, not one the project makes for them.

### A `/metrics` endpoint with Prometheus

Rejected because it answers a different question. A `/metrics` endpoint would give good time series for queue depth, job rates, and disk headroom, and it would make SLO evaluation straightforward — the one real weakness of the chosen approach. But it cannot answer "why did this chapter fail to download", which is the question the product actually generates, because a counter has no causal thread.

The architecture already decided against the endpoint, and this ADR agrees, with one caveat: if SLO measurement turns out to be the binding requirement, a `/metrics` endpoint plus Prometheus is a much cheaper answer than a full tracing pipeline, and it should be reconsidered before the LGTM stack is.

### A `vector` or `alloy` sidecar

Converting the existing log lines to metrics and shipping them to Loki is the natural next step, and the log schema was designed to make it possible — the `target: "metrics"` snapshots exist partly so they can be scraped later. Deferred rather than rejected, because it costs a container and is not needed while the rotation window suffices.

## Revisit this decision when

- **A second API instance is needed.** `grep` across one stream stops working, and this must be revisited together with ADR-0003 and ADR-0010.
- **SLO evaluation requires a time window.** That is the most likely trigger, and the cheapest response is a `/metrics` endpoint or a `vector` sidecar, not a full pipeline.
- **`jq` on rotated files becomes the bottleneck** during an investigation — the explicit condition the architecture draft named for revisiting.
- **The OTel Rust traces API, SDK, and OTLP exporter reach stable.** That removes the maturity objection and makes the `otlp` feature a low-risk option rather than a deferred one.
- **A regression needs history older than the rotation window.** That is a retention requirement, and retention is the one thing this design genuinely cannot provide.

## References

- [OpenTelemetry Rust](https://opentelemetry.io/docs/languages/rust/) and [`opentelemetry-rust` repository](https://github.com/open-telemetry/opentelemetry-rust) — component stability
- [`opentelemetry`](https://crates.io/crates/opentelemetry) — 0.33.0 published; pin 0.32 per the callout
- [`tracing-opentelemetry`](https://crates.io/crates/tracing-opentelemetry) — 0.33.0, requires `opentelemetry ^0.32`
- [`axum-tracing-opentelemetry`](https://crates.io/crates/axum-tracing-opentelemetry) — 0.39.1, requires `opentelemetry ^0.32`
- [`opentelemetry-appender-tracing`](https://crates.io/crates/opentelemetry-appender-tracing) — needed if log export is ever wanted; `tracing-opentelemetry` does not export logs
- [W3C Trace Context](https://www.w3.org/TR/trace-context/)
- [Building a complete LGTM stack with OpenTelemetry](https://oneuptime.com/blog/post/2026-02-06-lgtm-stack-opentelemetry/view) — directional; single-node sizing and the collector memory limiter
