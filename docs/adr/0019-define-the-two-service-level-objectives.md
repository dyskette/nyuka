# ADR-0019: Define the two service level objectives

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-22 |
| **Deciders** | dyskette |
| **Applies to** | `crates/jobs` (measurement), `crates/api` (emission), runbook |
| **Supersedes** | None |

This article defines the two objectives [ADR-0003](0003-run-the-job-queue-in-postgres-and-in-process.md) and [ADR-0014](0014-keep-backend-telemetry-as-structured-logs.md) deferred, states how each is measured without an aggregation stack, and records what one of them settles: whether PostgreSQL `LISTEN`/`NOTIFY` is ever needed.

## Context

Two objectives were named and left undefined. Leaving them undefined has a cost beyond the obvious one: ADR-0003 says time-to-first-page is *the input that decides whether `NOTIFY` is ever needed*, so an undefined objective leaves a whole piece of infrastructure permanently arguable. ADR-0014 says that if either objective needs window-based evaluation, that is evidence for adding `vector` and Loki — so the definition also decides whether two containers appear on a single-VM deployment.

An objective is therefore not paperwork here. Each one closes a question that is otherwise reopened every time someone looks at the job engine.

### What makes a number worth having

A target that cannot be missed is decoration, and a target that is always missed is ignored. Both failure modes are reachable from the same mistake: measuring something the service does not control.

A job fails for two unrelated reasons. The disk fills, the database is unreachable, the packaging step has a bug — those are this service's faults and they are fixable. A source returns 404 for a chapter the site removed, blocks the deployment's address, or requires a host capability this build does not implement — those are not faults at all, and no amount of attention to this codebase changes them. Counting both in one number means the first time a popular source has a bad week, the objective goes red and stays red, and the next person to look at it learns that the number does not mean anything.

### Measurement has to survive log rotation

ADR-0014 keeps telemetry as JSON on stdout with a `50m × 10` rotation. Computing a monthly rate by reading a month of request lines does not work: the lines are gone. Whatever is measured has to be summarized into the stream at a cadence that survives, or it cannot be evaluated over the window it is defined on.

## Decision

Two objectives, both evaluated from a periodic snapshot line rather than from an aggregation stack.

### SLO-1 — Job success rate

**99.9% of service-attributable job outcomes succeed, measured monthly.**

A job outcome counts against the objective when it fails for a reason inside this system:

| Counts | Excluded |
|---|---|
| `DomainError::Storage` — database or filesystem | `DomainError::Source` — the site was down, blocked us, or removed the chapter |
| `DomainError::InsufficientStorage` — the library volume is full | `DomainError::UnsupportedCapability` — the package needs a host capability this build does not implement |
| `DomainError::Internal` — a bug | |
| `DomainError::Invalid` or `Conflict` on a job payload — the enqueuing code produced something unrunnable | |
| A panicking handler, or a job released by the drain and never retried | |

Excluded outcomes are **counted and reported separately**, not discarded. The gap between the two rates is its own signal: a widening gap means the configured sources are rotting, which is a real thing to act on and a different action from fixing a bug.

> [!NOTE]
> 99.9% is only coherent alongside the exclusion. At a few thousand jobs a month it allows roughly three failures. With source faults counted, a single unreachable site exhausts that in an afternoon, and the objective would be red permanently from the first week.

`UnsupportedCapability` is excluded because it is not a failure: [ADR-0004](0004-run-manga-sources-as-aidoku-wasm-extensions.md) requires the capability check to happen at *install*, so reaching it during a job means the check was bypassed — which is a bug, and will surface as `Internal` rather than as this.

### SLO-2 — Time to first page

**95% of user-requested chapter downloads are readable within 60 seconds, measured monthly.**

The clock starts when `POST /api/v1/downloads` is accepted and stops when the chapter's archive is placed and its download recorded — the first moment the reader can open it.

The measurement covers **user-requested downloads only**, distinguished by the priority the enqueue used: a download someone is waiting for is enqueued at priority `0`, one discovered by a follow check at `5`. Nobody is waiting for the second, and including it would measure the scheduler's pacing rather than a person's experience.

#### This settles `NOTIFY`

The budget includes queue pickup, which is what makes the question answerable. A worker polls on a one-second interval, so polling can contribute at most one second to a sixty-second budget — under two percent.

`LISTEN`/`NOTIFY` optimizes exactly that interval, and buys at most 2% of a budget that is otherwise spent on someone else's web server. It is therefore **not needed**, and this is now a measured conclusion rather than an assumption. Revisit only if the p95 approaches 60 seconds *and* the breakdown shows pickup rather than fetching as the cause — which would mean the queue is saturated, and at that point more workers is the answer before `NOTIFY` is.

### Measurement: an hourly snapshot line

The process keeps counters and a duration sample in memory, and the scheduler emits one line per window:

```json
{"target":"nyuka_metrics","window_secs":3600,
 "jobs_succeeded":412,"jobs_failed_service":1,"jobs_failed_source":37,
 "ttfp_samples":38,"ttfp_p50_ms":18400,"ttfp_p95_ms":52100,"ttfp_max_ms":71200}
```

This is the middle option deliberately. Deriving percentiles by `jq` over raw lines cannot work across a month that has rotated away; adding `vector` and Loki reverses ADR-0014's container-count decision for a service with one tenant. A summary line is log-only, survives rotation as history, and makes the monthly evaluation a `jq` sum over at most 744 lines.

> [!IMPORTANT]
> A snapshot reports whether its sample was truncated. A percentile computed from a silently capped sample is worse than no percentile, because it reads as authoritative. If `ttfp_truncated` is ever `true`, the p95 in that line is a lower bound and must be read as one.

## Consequences

### What you gain

- **`NOTIFY` is closed**, with a reason rather than an omission. The next person to raise it has a number to argue against.
- **`vector` and Loki stay unbuilt**, and ADR-0014 stands rather than being quietly superseded.
- **The objectives are actionable.** Every outcome counted against SLO-1 has an action; none of them is "wait for someone else's site to come back".
- **Source rot becomes visible** as the gap between the two rates, which no single number would have shown.
- **Monthly evaluation is one `jq` expression** over lines that survive rotation.

### What it costs you

- **An in-process histogram is real code**, with a bounded sample and a truncation flag. It is state that has to be correct and reset cleanly per window, and it is the kind of code that is easy to get subtly wrong in a way tests do not notice.
- **The window is hourly, so the resolution is hourly.** A five-minute outage inside a good hour is invisible in the summary. The raw lines still show it while they exist, which is the trade.
- **Restarts lose the partial window.** A process that restarts at minute 50 discards 50 minutes of counters. At this cadence and volume that is noise; at ten-minute windows it would not be.
- **Classification has to be maintained.** A new `DomainError` variant defaults to counting against the objective unless someone decides otherwise, which is the right default and still a decision that has to be made each time.
- **99.9% is tight enough that three bugs in a month miss it.** That is the point, and it means the number will sometimes be red for a reason that is genuinely worth the interruption.

### Follow-up work this decision creates

1. **Implement the counters and the sample** in `crates/jobs`, with the classification above and an explicit truncation flag.
2. **Emit the snapshot from the scheduler**, on its own `nyuka_metrics` target so it can be filtered or routed separately.
3. **Measure the priority split.** SLO-2 depends on user-requested downloads being enqueued at priority `0` and follow-discovered ones at `5`. That is a convention in two call sites today; it should be a named constant that both use.
4. **Write the `jq` recipes** for the monthly evaluation into the runbook, alongside the ones ADR-0014 already asks for.
5. **Review the 60-second budget after a month of real data.** It is an estimate; the snapshot is what turns it into a measurement.

## Alternatives considered

| Alternative | Outcome |
|---|---|
| Count every job failure in SLO-1 | Rejected. One flaky source makes the objective permanently red, which teaches everyone to ignore it. |
| Time-to-first-page as "open an already-downloaded chapter" | Rejected. Measures a local file read in milliseconds and says nothing about the job engine, so it cannot answer the `NOTIFY` question that made this objective worth defining. |
| Time-to-first-page as catalog browse latency | Rejected. Dominated by the third-party site, so the number measures someone else's infrastructure and the only available fix is "use a different source". |
| `jq` over raw lines, no snapshot | Rejected. A monthly window outlives a `50m × 10` rotation, so the lines the query needs are gone. |
| `vector` plus Loki | Rejected for now. Two containers on a single-VM deployment for a single-tenant service, reversing ADR-0014's central trade. Revisit if alerting on a rolling window becomes something you actually want, rather than to satisfy a definition. |
| 95% success rate | Rejected as too loose. At a few thousand jobs a month it tolerates around 150 failures, which is enough to hide a genuine regression. |
| Defer the targets until a month of data exists | Rejected. It is the honest option and also indefinitely deferrable, which is how this item got old. The numbers here are estimates with a scheduled review, which is a better state than undefined. |
