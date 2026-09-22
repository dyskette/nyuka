# ADR-0002: Use SeaORM for persistence

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/persistence`, `crates/jobs` |
| **Supersedes** | None |

This article explains why the persistence layer uses [SeaORM](https://www.sea-ql.org/SeaORM/) rather than SQLx or Diesel, what you give up by choosing an ORM over compile-time verified SQL, and the CI check that compensates for it.

## Context

`crates/persistence` implements the repository ports declared in `crates/domain`. PostgreSQL is the only supported database: it holds domain data, the job queue, sessions, and the per-source key-value namespace. The following constraints were fixed before the persistence library was selected.

### The library must be confinable to one crate

`crates/domain` declares ports as traits and must compile without any I/O dependency. Persistence types may not appear in domain signatures. This rules nothing out on its own, but it means you pay a mapping layer between database rows and domain entities regardless of which library you choose. A library whose types are designed to be the domain model works against the architecture rather than with it.

### Every database call is awaited from a multi-threaded runtime

Per [ADR-0001](0001-use-axum-for-the-http-api.md), the process runs on the Tokio multi-threaded runtime. Repository methods are called from Axum handlers and from job workers, both of which need `Send` futures. A synchronous database library requires either `spawn_blocking` around every call or a separate async adapter crate.

### The job queue needs row-level locking and `jsonb`

The job engine claims work with this pattern:

```sql
UPDATE job SET state = 'running', locked_by = $1, locked_at = now()
WHERE id = (
    SELECT id FROM job
    WHERE state = 'queued' AND run_at <= now()
    ORDER BY priority, run_at
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
RETURNING *
```

The `job.payload` column is `jsonb` and holds per-kind payloads plus the propagated trace context. The library must express `FOR UPDATE SKIP LOCKED`, or get out of the way so you can write the statement yourself.

### Reads traverse relations

The domain model is a chain of related tables: `source_repo` → `source` → `manga` → `chapter` → `downloaded_chapter`, with `follow`, `job`, `user`, `session`, and `source_kv` alongside. Library and catalog reads fetch a series with its chapters and download state in one request. Every list endpoint uses cursor pagination, and several accept optional filters, which means the generated SQL varies by request.

### Migrations run on boot, and the schema is owned by the application

The container runs pending migrations at startup, and `/readyz` reports whether migrations are current. Migrations must be embedded in the binary and callable programmatically.

### Query logging is part of the observability contract

The observability design requires statement logging off by default, slow statements logged above a 250 ms threshold, and each repository method instrumented with `db.system`, `db.operation`, and `db.sql.table` span attributes. It also reports `db.pool.in_use` in the 60-second metrics snapshot, so the connection pool must be introspectable.

## Decision

Use **SeaORM 2.x** for persistence, with these specifics:

- Pin **`sea-orm = "2.0"`** and **`sea-orm-migration = "2.0"`**, both with the `tokio` runtime feature. The migration crate's `async-std` support is deprecated in 2.0; do not use it.
- Define entities in `crates/persistence` using the `#[sea_orm::model]` format. Map them to domain entities at the repository boundary. Entities are an implementation detail of the adapter.
- Express the job claim with `lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)` where the DSL is clear, and with `query_one_raw` for the `UPDATE ... FROM (SELECT ... FOR UPDATE SKIP LOCKED)` form. Keep the claim query in exactly one function.
- Configure logging through `ConnectOptions`: `sqlx_logging(false)` to silence per-statement logs, and `sqlx_slow_statements_logging_settings(LevelFilter::Debug, Duration::from_millis(250))` for the slow-query threshold.
- Run migrations on boot with `Migrator::up`, and expose migration status to `/readyz`.

> [!IMPORTANT]
> SeaORM does not verify SQL against the live schema at compile time. The entity-versus-schema drift check described under [Follow-up work](#follow-up-work-this-decision-creates) is not optional hygiene — it is the control that makes this decision safe. Implement it in the same change that adds the first entity.

## Consequences

### What you gain

- **Async without an adapter.** SeaORM is async-native and sits on SQLx 0.9. Repository futures are `Send`, so they compose with Axum handlers and job workers directly.
- **Relation traversal is declarative.** SeaORM 2.0 declares relations on the `Model` struct with typed fields (`#[sea_orm(has_many)]`, `BelongsTo<Entity>`), and cardinality is encoded in the type. Loading a series with its chapters and download state does not require hand-written joins and row-folding for each read shape.
- **Dynamic queries stay type-checked.** Catalog listing, library listing, and the jobs endpoint all build predicates conditionally from query parameters. Because SeaORM composes queries as values through SeaQuery, an optional filter is a conditional `.filter()` call rather than a second code path.
- **The observability contract is satisfied by configuration, not workarounds.** `sqlx_slow_statements_logging_settings` provides the 250 ms threshold as a first-class setting, and `map_sqlx_postgres_pool_opts` exposes the underlying `PoolOptions` for pool sizing and metrics.
- **SQLx remains available underneath.** SeaORM does not hide its driver. When a query is better expressed as SQL, `query_one_raw` and `query_all_raw` take a statement and return rows, on the same pool and the same connection semantics. You are not choosing between an ORM and SQL; you are choosing the default and keeping the escape hatch.
- **Migrations are Rust and embedded.** `sea-orm-migration` defines migrations programmatically, which keeps them in the binary and callable at boot, and lets the schema-registry tooling generate tables in dependency order during development.

### What it costs you

- **No compile-time verification against the real schema.** This is the material cost of the decision. SeaORM type-checks queries against your *entity definitions*. If a migration adds a non-null column and the entity is not updated, the code still compiles and fails at runtime. SQLx and Diesel both catch that class of error during `cargo build`.

  > [!NOTE]
  > Treat entity definitions as a second source of truth that must be reconciled with migrations in CI. The drift check below converts a runtime failure class into a build failure, which is most of what the alternatives were offering.

- **SeaORM 2.0 is young as a stable release.** It reached stable on 27 July 2026 after 43 release candidates. Expect some ecosystem crates to still target the 1.1.x line, and expect to read changelogs on patch releases for longer than usual.
- **You are absorbing the 2.0 API changes up front.** Building on 2.x rather than 1.1.x means the following apply from the start:

  | Change in 2.0 | What it affects |
  |---|---|
  | `execute`, `query_one`, `query_all`, and `stream` take SeaQuery statements; raw-SQL forms moved to `execute_raw`, `query_one_raw`, `query_all_raw`, `stream_raw` | The job-claim query and any hand-written SQL |
  | `ExprTrait` must be in scope for expression methods such as `eq`, `add`, and `contains` | Every filter-building call site |
  | PostgreSQL auto-increment uses `GENERATED BY DEFAULT AS IDENTITY` instead of `serial` | Initial migration definitions |
  | `async-std` deprecated in the migration crate | Runtime feature selection |

  These are one-time costs on a greenfield codebase and would be a migration on an existing one. Starting on 2.x is the cheaper moment.

- **Four layers between your call and the wire.** SeaORM builds on SeaQuery 1.0, which builds on SQLx 0.9, which builds on the Postgres driver. When generated SQL is wrong or slow, you need statement logging turned on temporarily to see what was actually sent. Plan for that during performance work.
- **Two model layers to maintain.** Each aggregate has a SeaORM `Model` and `ActiveModel` plus a domain entity, with mapping between them. The hexagonal boundary requires this regardless of library, but an ORM makes the duplication more visible and more tempting to collapse. Do not collapse it.

### Follow-up work this decision creates

1. **Add the schema drift check to CI.** Start an ephemeral PostgreSQL service, run `Migrator::up`, regenerate entities with `sea-orm-cli generate entity`, and fail the build on any diff against the committed entities. This is the control referenced in the Decision section.
2. **Centralize `ConnectOptions`.** One constructor builds the pool with sizing, timeouts, `sqlx_logging(false)`, and the 250 ms slow-statement threshold, so no code path creates a differently configured pool.
3. **Instrument repository methods.** Apply `#[tracing::instrument]` with `db.system = "postgresql"`, `db.operation`, and `db.sql.table` fields to every port implementation, and skip arguments that may contain secrets or large payloads.
4. **Write a concurrency test for the job claim.** Run N workers against a seeded `job` table and assert that every row is claimed exactly once. This is the query least protected by the type system and the one where a mistake is most expensive.
5. **Pin and group updates.** Constrain `sea-orm`, `sea-orm-migration`, and `sea-orm-cli` to matching `2.0.x` requirements and group them in one Renovate rule so the CLI never drifts from the library that generated the entities.

## Alternatives considered

| Alternative | Version (Sept 2026) | Compile-time SQL verification | Async model | Outcome |
|---|---|---|---|---|
| SeaORM | 2.0.x | No, entity-level only | Native | **Chosen** |
| SQLx | 0.9.x | Yes, against cached schema metadata | Native | Rejected. Verification does not extend to the dynamic queries this API is built from. |
| Diesel | 2.3.x + `diesel-async` 0.7.x | Yes, against `schema.rs` | Sync core, async via companion crate | Rejected. Async support depends on a pre-1.0 companion crate. |
| `tokio-postgres` directly | — | No | Native | Rejected. Pooling, migrations, and all mapping become project code. |

All three primary candidates express `FOR UPDATE SKIP LOCKED` in their query DSL, so the job queue did not decide this. SeaORM and Diesel expose it as a builder method, and in SQLx you write the SQL. That requirement is neutral.

### SQLx

SQLx is the strongest alternative, and its central advantage is real: the `query!` family checks your SQL against the actual database at compile time, including column types and nullability. The workflow is mature — `cargo sqlx prepare` writes a `.sqlx` directory that you commit, CI builds with `SQLX_OFFLINE=true` and no database, and `cargo sqlx prepare --check` fails the build when the cached metadata falls behind the queries or the schema. That is a stronger guarantee than the drift check adopted here, because it verifies each query rather than the entity set.

It was rejected for two reasons:

1. **The verification does not cover the queries that need it most.** The macros require the query to be a string literal or a concatenation of literals. Dynamic SQL is out of scope by design. But the source catalog endpoint takes `?q=&filters=&cursor=`, the library and jobs endpoints take optional state filters, and every list endpoint is cursor-paginated. Those queries would be built with `QueryBuilder` at runtime, which is unverified. You would get compile-time checking on the simple lookups and lose it on exactly the complex, frequently-changed queries where a schema mistake is most likely.
2. **Relation loading becomes project code.** With roughly a dozen related tables and several read shapes that span three or four of them, every join and every row-folding step is hand-written and hand-maintained. That is a reasonable trade in a service with a handful of query shapes. It is a poor one here.

Note that this is not an exclusive choice. SeaORM 2.0 runs on SQLx 0.9, so SQLx is a transitive dependency either way, and `query_one_raw` is available whenever plain SQL is the better tool.

### Diesel

Diesel offers the strongest compile-time guarantees of the three, verifying queries against a generated `schema.rs`, and Diesel 2.3.x is stable and carefully maintained.

It was rejected primarily on the async question. Diesel's core is synchronous. Async support comes from `diesel-async`, currently on **0.7.x**, which would place the entire persistence layer of this project on a pre-1.0 companion crate — specifically to avoid an ORM that is already async. The alternative is wrapping every repository call in `spawn_blocking`, which adds a thread hop per query and complicates the transaction story, since a transaction must stay on one connection and therefore one blocking task.

Diesel's type signatures are also the heaviest of the three. The conditional filters this API needs require boxed queries, which discards part of the compile-time benefit that justified choosing Diesel in the first place.

### Direct driver use

Using `tokio-postgres` with `deadpool` and a migration crate was considered and rejected quickly. It trades a well-tested library for project-specific code in pooling, migration ordering, and row mapping, with no compile-time verification to show for it. The only scenario that favors it is needing driver features no higher-level library exposes, which this design does not.

## Revisit this decision when

- The CI drift check fails to catch an entity-versus-schema mismatch that reaches production. That is direct evidence the chosen control is weaker than compile-time verification, and it should reopen the SQLx comparison.
- Query shapes shift from record access toward reporting or analytics SQL. Hand-written verified SQL wins on that workload, and the ORM stops paying for itself.
- Generated SQL appears in a profile as the cause of a latency problem that cannot be fixed by indexing or by a targeted `query_all_raw` call.
- SeaORM 2.x releases stall, or the project's release cadence and issue response degrade materially.

## References

- [Announcing SeaORM 2.0](https://www.sea-ql.org/blog/2026-07-27-sea-orm-2.0/)
- [SeaORM 2.0 migration guide](https://www.sea-ql.org/blog/2026-01-12-sea-orm-2.0/)
- [SeaORM changelog](https://docs.rs/crate/sea-orm/latest/source/CHANGELOG.md)
- [`sea_orm::ConnectOptions`](https://docs.rs/sea-orm/latest/sea_orm/struct.ConnectOptions.html)
- [`sea_orm::QuerySelect::lock_with_behavior`](https://docs.rs/sea-orm/latest/sea_orm/query/trait.QuerySelect.html)
- [`sqlx::query!` macro and offline verification](https://docs.rs/sqlx/latest/sqlx/macro.query.html)
- [SQLx repository](https://github.com/launchbadge/sqlx)
- [Diesel changelog](https://diesel.rs/changelog.html)
- [`diesel-async`](https://crates.io/crates/diesel-async)
- [`diesel::query_dsl::methods::ModifyLockDsl`](https://docs.diesel.rs/diesel/query_dsl/methods/trait.ModifyLockDsl.html)
