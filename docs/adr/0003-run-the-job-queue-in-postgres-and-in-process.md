# ADR-0003: Run the job queue in PostgreSQL, in process

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/jobs`, `crates/api` |
| **Supersedes** | None |

This article explains why background work runs on a PostgreSQL-backed queue inside the API process, rather than on Redis or a dedicated broker with a separate worker container. It also explains why the queue is written against the existing database layer instead of adopting a job-queue crate, and what you accept by keeping everything in one process.

## Context

The job engine runs chapter downloads, chapter packaging, metadata refreshes, follow checks, and source updates. Three properties of this workload shape the decision.

### Enqueueing must be atomic with the domain write that caused it

`POST /downloads` accepts a set of chapter IDs and an optional `run_at`, returns `202 Accepted` with a `Location` header, and carries an `Idempotency-Key`. The endpoint writes domain rows and enqueues jobs for the same request. If those two writes can diverge, you get one of two failures:

- The domain write commits and the enqueue fails. The user sees an accepted download that never starts.
- The enqueue succeeds and the domain write rolls back. A worker claims a job referring to a row that does not exist.

This is the dual-write problem. The `job` table's unique `idempotency_key` constraint is also what makes a retried `POST /downloads` idempotent, which only works if the constraint lives in the same database as the request's other writes.

### Rate limiting toward source sites is per-process state

Source sites ban clients that fetch too aggressively. `download_chapter` fans out page fetches inside the handler under a `Semaphore` scoped to the source, defaulting to 2–4 concurrent requests. The Aidoku runtime also caches FlareSolverr clearance cookies and user-agent strings per domain, and `wasmtime` caches compiled modules on disk per process.

A semaphore bounds concurrency within one process. Two processes with the same semaphore configuration produce twice the request rate toward the site.

### Job progress must reach the browser

Job handlers publish `JobEvent` values to a `tokio::sync::broadcast` channel. The SSE handler for `GET /api/v1/events` subscribes to that channel and streams `job.progress`, `job.state`, and `chapter.new` events. Per [ADR-0001](0001-use-axum-for-the-http-api.md), SSE is the only live-state channel; the frontend has no polling fallback.

A `broadcast` channel is in-process memory. If workers run elsewhere, progress needs a transport back to whichever process holds the SSE connection.

### Other constraints

- **Deployment is a single Docker Compose host.** The target is one VM running `api`, `postgres:17`, `flaresolverr`, and optionally `caddy`. Every added container costs RAM on that VM and a healthcheck, secret, and backup story.
- **Scheduling is coarse.** Deferred downloads are jobs with a future `run_at`. Follow checks run on intervals measured in hours. A scheduler task ticks once per minute.
- **Volume is low.** Follow checks plus chapter downloads for a personal library produce jobs at a rate measured in tens to low hundreds per hour, with individual jobs lasting seconds to minutes because they are network-bound.
- **Persistence is already solved.** Per [ADR-0002](0002-use-seaorm-for-persistence.md), SeaORM 2.x on SQLx 0.9 owns the connection pool, migrations, and instrumentation.

## Decision

Run the queue as a PostgreSQL table consumed by Tokio worker tasks inside the API process.

- **Storage.** One `job` table with `(id, kind, payload jsonb, state, priority, run_at, attempts, max_attempts, locked_by, locked_at, last_error, idempotency_key)`, a unique index on `idempotency_key`, and a partial index on `(priority, run_at) WHERE state = 'queued'`.
- **Claim.** `UPDATE ... WHERE id = (SELECT id ... FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *`, on the SeaORM pool, in one function.
- **Workers.** A bounded pool of `tokio::spawn` tasks in the same process as the HTTP server. WASM invocations go to `spawn_blocking`; page fetches run under the per-source `Semaphore`.
- **Wake-up by polling.** Workers poll on a short fixed interval with jitter. Do not add `LISTEN`/`NOTIFY` in v1.
- **Write the queue against SeaORM directly.** Do not adopt a job-queue crate. See [Alternatives](#alternatives-considered) for the version analysis behind this.
- **Recovery.** On startup, reset `running` jobs with a stale `locked_at` to `queued`. On `SIGTERM`, stop claiming, drain in-flight jobs up to a bounded timeout, then exit.

> [!IMPORTANT]
> This design assumes exactly one API instance. The per-source semaphore, the `broadcast` fan-out, and the stale-lock recovery heuristic all depend on it. Running a second instance is not a scaling knob you can turn — it is the trigger to revisit this ADR.

### Why polling rather than `LISTEN`/`NOTIFY`

`NOTIFY` is a latency optimization, not a source of truth: notifications are lost when no listener is connected, so a correct implementation polls anyway and treats notifications as an early wake-up. That means `NOTIFY` adds a second mechanism without removing the first, and it carries real costs:

- Every commit carrying a notification takes a global lock that serializes commits on the instance.
- Each listener needs a dedicated connection held outside the application pool.
- `LISTEN` does not survive a transaction-mode connection pooler, which constrains future deployment options.

The benefit is shaving the poll interval off job start latency. For a download that then spends seconds to minutes fetching pages, that is not worth a second mechanism. Revisit only if a time-to-first-page SLO requires it.

## Consequences

### What you gain

- **Transactional enqueue, for free.** Domain writes and job inserts commit in one transaction. There is no outbox table, no reconciliation job, and no window where the two systems disagree. The `idempotency_key` unique constraint is enforced by the same database that holds the chapter rows.
- **Deferred work is a `WHERE` clause.** `run_at <= now()` handles both scheduled downloads and follow-check intervals. No delayed-queue primitive, no separate scheduler service.
- **The queue is inspectable with SQL.** `GET /jobs?state=&cursor=` is an ordinary paginated query over a table. Debugging a stuck job is a `SELECT`, and the 60-second metrics snapshot computes `jobs.queued`, `jobs.running`, `jobs.failed_1h`, and `jobs.p95_duration_ms` from the same table.
- **Rate limiting stays correct by construction.** One process means one semaphore per source, so the configured concurrency cap is the actual request rate toward the site.
- **Progress reaches the browser without a transport.** Handlers and the SSE endpoint share a `broadcast` channel in the same process. Job spans also link to the enqueuing trace through `job.payload.trace_context` without crossing a process boundary.
- **One backup, one image, one deployment.** The existing `pg_dump` sidecar captures jobs and domain data at the same point in time. Rollback stays "redeploy the previous tag."
- **No added container.** Nothing new to run, secure, healthcheck, or size on the VM.

### What it costs you

- **Workers and request handling share a process.** A burst of downloads or a pathological WASM module competes with request latency. The mitigations are already in the design and must stay in it: WASM calls on `spawn_blocking` so they never occupy an async worker thread, memory caps and epoch-based timeouts on `wasmtime`, a bounded worker count, and per-source concurrency caps.
- **You cannot scale out without redesigning.** Horizontal scaling requires distributed rate limiting, a pub/sub transport for SSE, and lease-based lock recovery. Accept single-instance operation as a property of v1, not a temporary state.

  > [!NOTE]
  > This deviates from the organizational default of multi-AZ production deployment. The deployment target is a single self-hosted VM for a single-tenant library, so the availability requirement that rule protects does not apply. Record this as an accepted deviation rather than an oversight, and re-evaluate it together with the scaling trigger below.

- **You own the correctness of the queue mechanics.** Exponential backoff with jitter, the attempt ceiling, stale-lock recovery, cancellation semantics, and drain-on-shutdown are project code. A library would have supplied tested versions of all of them.
- **Polling adds a small constant latency and a constant query load.** One claim query per worker per interval, on a partial index, against an otherwise idle database. Negligible here, but it is a floor that does not go away.
- **Table maintenance is your responsibility.** A high-churn `job` table needs per-table autovacuum tuning and a retention policy for terminal rows, or the partial index bloats and claim latency drifts upward.

### Follow-up work this decision creates

1. **Write the concurrency test first.** Run N workers against a seeded `job` table and assert each row is claimed exactly once, then assert that a worker killed mid-job has its row recovered by the stale-lock sweep. This is the least type-checked code in the system.
2. **Tune the `job` table.** Set a lower `autovacuum_vacuum_scale_factor` on `job`, add the partial index on queued rows, and add a retention job that deletes terminal rows older than a configured window.
3. **Bound the worker pool from configuration** and validate it at startup alongside the rest of the config, so worker count and per-source concurrency are never implicit.
4. **Add a shutdown test** asserting that `SIGTERM` stops claiming, drains in-flight work, and exits within the bounded timeout, and that undrained jobs return to `queued`.
5. **Define the two SLOs the architecture defers:** job success rate and time-to-first-page. The second one is the input that decides whether `NOTIFY` is ever needed.
6. **Record the cancellation semantics.** The architecture leaves open whether cancelling a running download deletes partial files or keeps them for resume. Decide it here, because it determines whether cancellation is cooperative or immediate.

## Alternatives considered

| Alternative | Status (Sept 2026) | Outcome |
|---|---|---|
| Redis-backed queue | Mature | Rejected. Reintroduces the dual-write problem; needs an outbox to fix it. |
| RabbitMQ or NATS | Mature | Rejected. Same atomicity problem, larger operational surface. |
| Separate worker container, same Postgres queue | — | Rejected. Breaks per-source rate limiting and the SSE fan-out. |
| `apalis` / `apalis-postgres` | 1.0.0-rc.9, rc line active | Rejected for now. Pre-release; strongest candidate to revisit. |
| `apalis-sql` (stable line) | 0.7.4, `sqlx ^0.8.1` | Rejected. SQLx version conflicts with SeaORM 2.0. |
| `sqlxmq` | 0.6.0, last published May 2025 | Rejected. Unmaintained in practice; `sqlx ^0.8`. |
| `underway` | 0.2.0, last published Jul 2025 | Rejected. Same as `sqlxmq`. |

### Redis, or a dedicated broker

Redis is the conventional answer, and it is genuinely better at three things: job start latency measured in single-digit milliseconds, pub/sub fan-out that would carry SSE events across processes, and throughput far beyond what a Postgres queue sustains.

None of those are binding constraints here. Job start latency is irrelevant when the job itself takes seconds to minutes. Pub/sub fan-out is only needed if workers move out of process, which is the thing being decided. And the published guidance for graduating from Postgres to Redis puts the threshold around 100,000 jobs per hour or a hard sub-millisecond latency requirement — three orders of magnitude above this workload.

The decisive argument against it is atomicity. A job in Redis referring to a row in Postgres has no transaction spanning both. The standard fix is the transactional outbox pattern: write the intent to a Postgres table inside the business transaction, then have a relay forward it to the broker. That means you build a Postgres queue *and* run Redis *and* maintain a relay. Adding Redis makes the system strictly larger and strictly less correct at the point that matters most.

RabbitMQ and NATS were rejected on the same atomicity grounds, with more operational surface and no benefit this workload can use.

### A separate worker container sharing the Postgres queue

This keeps transactional enqueue — the API still inserts jobs in its own transaction — and is the option worth the most consideration, because it isolates download work from request latency and is the conventional shape for this kind of system.

It was rejected on three specific costs:

1. **Rate limiting becomes distributed.** The per-source semaphore no longer bounds anything, because two processes each hold their own. Respecting source rate limits would require a shared limiter, most likely a Postgres advisory lock or a leases table, built and tested by this project. Getting it wrong means IP bans on the sites the application exists to read.
2. **Progress needs a transport.** Workers would have to publish `JobEvent` back to whichever process holds the browser's SSE connection, via `LISTEN`/`NOTIFY` or Redis pub/sub. That reintroduces the component this ADR avoids, to serve the only live channel in the product.
3. **The runtime gets duplicated.** The worker needs its own `wasmtime` engine, its own compiled-module cache, its own copy of the installed `.aix` files, and its own FlareSolverr clearance cookie cache. That is duplicated memory, duplicated warm-up, and a second place where source state can diverge.

The isolation benefit is real but addressable in-process: `spawn_blocking` keeps WASM off the async worker threads, and wasmtime's memory and epoch limits bound a misbehaving source. That is a cheaper answer than a second process.

### Job-queue crates

The dependency data is what settles this, and it cuts in a direction worth recording precisely.

- **`apalis` is the strongest candidate and nearly fits.** `apalis-postgres` 1.0.0-rc.9, published 16 September 2026, depends on `sqlx ^0.9`, which unifies with SeaORM 2.0's SQLx version. It is built on `tower::Service`, so its middleware model matches the one already chosen in ADR-0001, and it offers `PostgresStorageWithListener` for `NOTIFY`-based wake-up. It is rejected only because the 1.0 line is still a release candidate, with `apalis-core` moving to `rc.10` while `apalis-postgres` sits at `rc.9`. Building the job engine — the subsystem carrying every download — on a churning pre-release is not a good trade for code that amounts to one table and three queries.
- **The stable `apalis` line does not fit.** `apalis-sql` 0.7.4 depends on `sqlx ^0.8.1`, which is semver-incompatible with SeaORM 2.0's `sqlx 0.9`. Adopting it means two SQLx versions in the tree, two connection pools, and no shared transaction with domain writes — which forfeits the main reason for choosing Postgres in the first place.
- **`sqlxmq` and `underway` are effectively unmaintained.** `sqlxmq` 0.6.0 last published in May 2025 and `underway` 0.2.0 in July 2025, both on `sqlx ^0.8`. They carry the same version conflict plus staleness.

There is also a scope argument independent of versions. The handler behavior this project needs — per-source semaphores, page fan-out inside a single job, cancellation tied to partial-file cleanup, `Idempotency-Key` values originating in the HTTP layer, and trace-context linkage into `job.payload` — lives in handlers, not in the queue. What a library would supply is the claim loop, backoff, and recovery, which is a bounded amount of code guarded by the tests listed above.

## Revisit this decision when

- **A second API instance is needed.** This invalidates the semaphore, the `broadcast` fan-out, and the stale-lock heuristic simultaneously. Treat it as a redesign of this ADR, not a configuration change.
- **`apalis` 1.0 ships stable on `sqlx 0.9`.** It would then be a maintained, tower-native queue that unifies with the database layer, and the hand-rolled claim loop becomes code to delete rather than code to own.
- **Job start latency appears in a time-to-first-page SLO breach.** That is the evidence that would justify `LISTEN`/`NOTIFY`, and it should be measured before it is implemented.
- **Download work measurably degrades request latency** after the `spawn_blocking` and wasmtime limits are verified to be in effect. That is the evidence for a separate worker container, and it also means building the distributed rate limiter.
- **Claim latency drifts upward** despite the partial index, which points at table bloat and retention rather than at the queue design.

## References

- [`apalis-postgres` on crates.io](https://crates.io/crates/apalis-postgres)
- [`apalis` on crates.io](https://crates.io/crates/apalis)
- [`sqlxmq` on crates.io](https://crates.io/crates/sqlxmq)
- [PostgreSQL 17: `SELECT ... FOR UPDATE ... SKIP LOCKED`](https://www.postgresql.org/docs/17/sql-select.html#SQL-FOR-UPDATE-SHARE)
- [PostgreSQL 17: `NOTIFY`, including the 8000-byte payload limit](https://www.postgresql.org/docs/17/sql-notify.html)
- [Scaling Postgres LISTEN/NOTIFY](https://pgdog.dev/blog/scaling-postgres-listen-notify) — directional; describes the commit-time global lock
- [Build a Postgres job queue with SKIP LOCKED](https://www.prisma.io/blog/you-dont-need-a-job-queue-postgres-already-has-skip-locked) — directional; dual-write and outbox discussion
- [Postgres as a job queue vs Redis vs SQS](https://dev.to/libme/postgres-as-a-job-queue-vs-redis-vs-sqs-when-does-just-use-your-database-stop-working-1f24) — directional; graduation thresholds
