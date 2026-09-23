# ADR-0020: Return read models from list endpoints

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-23 |
| **Deciders** | eddy.castillo@mobiik.com |
| **Applies to** | `crates/domain` (`model.rs`, `ports.rs`), `crates/persistence`, `crates/jobs`, `crates/api/src/routes` |
| **Supersedes** | None |

Every list endpoint returns a projection that carries the joined values its screen displays, not the entity a row holds. This article explains why the entity DTOs were not enough, what the projections cost to maintain, and where the boundary between the two sits.

## Context

### A list of foreign keys answers nothing

Four screens were built against endpoints that returned entities faithfully, and all four were unusable:

| Screen | Endpoint returned | What the screen shows |
|---|---|---|
| Library | `MangaDto` with `source_id` | The source's name, chapter count, downloaded count |
| Detail panel | `ChapterDto` | Whether each chapter has a file |
| Downloads | `JobDto` with no payload | Which chapter a download is for |
| Follows | `FollowDto` with `manga_id` | The series title, its source, chapters still missing |

Each of those values lives in another table. An entity DTO is correct about the row it represents and silent about everything a reader asks of a list of them.

### The client cannot fill the gap

Resolving the joins in the browser means one request per row. At the default page size of 50 that is 50 requests for a library screen, over a connection budget that ADR-0010 already spends one of on the event stream.

Nor can the client join against data it already holds. Both sides are keyset-paginated independently, so a follow on page 1 may name a series on page 3. A client-side join is correct only when both collections fit in one page, which is the case that does not need it.

### The payload is not available to publish

`JobDto` deliberately omits `payload`. It carries a trace context and, for some kinds, keys that belong to the source rather than the browser. Exposing it to let the client work out a job's subject would publish the rest to answer one question.

## Decision

A list endpoint whose screen displays a joined value returns a **read model**: a `struct` that owns the entity and the resolved values beside it.

The shape is the same in all four cases:

- The domain defines `XSummary { x: X, …resolved fields }` in `model.rs`. It is a projection, not an entity: it has no identity of its own and is never written.
- The port declares `list_summaries` **alongside** `list`, never replacing it. A caller that needs only the rows does not pay for the joins.
- The persistence adapter implements it as one query. `N+1` on the server is the same failure as `N+1` in the browser with a shorter round trip.
- The API exposes `XSummaryDto` with `#[serde(flatten)]` over the entity DTO, so the summary is a superset of the entity on the wire and a client reading the entity's fields keeps working.

Applied to `MangaSummary`, `ChapterSummary`, `JobSummary`, and `FollowSummary`.

> [!IMPORTANT]
> Never cast an untrusted value in SQL to make a join work. `JobSummary` resolves its subject from a JSONB payload, and `(payload->>'chapter_id')::uuid` fails the **whole statement** on one malformed row — so a single bad payload empties the queue view instead of showing one row without a subject. Guarding the cast with `AND` in the join condition does not help: Postgres does not promise to evaluate conjuncts in written order, and a regex guard written first still let the cast run. Parse the value in Rust, where a bad one is an `Option`.

## Consequences

### What you gain

- One request renders a screen. The library, the queue, and the follows list each cost one query regardless of page size.
- The join is written once, in SQL, where the database can use its indexes — rather than once per client in a language that cannot.
- The entity DTOs stay honest. `JobDto` still omits its payload; `MangaDto` still has no chapter count, because a series does not have one.
- A summary is a superset of its entity on the wire, so widening an endpoint from one to the other is not a breaking change.

### What it costs you

| Cost | Where it lands |
|---|---|
| A second method on the port, and a second implementation in every test fake | `ports.rs`, and the `unimplemented!` stubs in `crates/jobs/src/handlers/*` |
| Hand-written SQL that ADR-0002 already notes is not compile-time verified | `crates/persistence/src/repository.rs`, `crates/jobs/src/queue.rs` |
| Aggregate correctness that only a database test can check | `crates/persistence/tests/repository.rs` |
| A `#[serde(flatten)]` schema that renders as `allOf` in OpenAPI | `web/src/shared/api/schema.d.ts`, where it becomes an intersection type |

The aggregate cost is the one that bites. `COUNT(*)` where `COUNT(…) FILTER` was meant, or an `INNER JOIN` where `LEFT JOIN` was meant, produces a number that is plausible and wrong. Neither the type system nor a code review reliably catches it; only a test with a known answer does.

### Follow-up work this decision creates

1. Every read model has a database test asserting its aggregates against a known fixture, including the zero case — a series with no chapters, a follow with nothing missing. An `INNER JOIN` passes every non-zero test.
2. Every read model's test asserts the joined value, not only its presence. `missing_count` must be wrong-by-filter, not merely non-null.
3. When a fifth screen needs one, check first whether an existing summary covers it. Four is a pattern; nine would be a sign the entity DTOs are wrong.
4. Revisit `list_summaries` on `JobQueue`: it ignores its cursor, because the job list is capped at 100 rows. Wire keyset pagination when something needs the second page.

## Alternatives considered

| Alternative | Status | Outcome |
|---|---|---|
| Expand entity DTOs in place | — | Rejected. Makes every caller pay for joins it does not need. |
| Resolve joins in the client | — | Rejected. `N+1`, and impossible across pages. |
| A general `?expand=` parameter | — | Rejected. One endpoint with a combinatorial number of response shapes. |
| GraphQL | — | Rejected. Replaces a solved problem with a much larger one. |

### Expanding the entity DTOs in place

This is the smallest change and the strongest alternative. `MangaDto` gains `source_name`, `chapter_count`, and `downloaded_count`; nothing else moves; there is no second port method and no second test fake. The API surface shrinks rather than grows, and a client has one type per resource instead of two.

It was rejected on three counts, in order of weight.

First, it makes the cost unconditional. `GET /manga/{id}` is served on every detail-panel open and needs none of the aggregates; the `check_follow` handler loads a series per follow and needs none of them either. Both would pay for three joins to satisfy a screen neither renders.

Second, it makes a claim that is not true of the entity. "How many chapters" is a question about the database at a moment, not a property of a series — a series has the chapters its source published, whether or not this server has fetched them. Putting the count on `Manga` means every consumer of that type inherits a field that is only meaningful when it came from one particular query.

Third, it does not solve the job case at all. A job's subject is not a field of a job; it is the result of interpreting a payload whose shape depends on the kind. There is no honest way to put `manga_title` on `Job`.

### A general `?expand=` parameter

`GET /manga?expand=source,counts` looks like it generalises the decision and removes the second method. In practice each combination is a distinct response type, so either the schema describes every one of them or it describes none precisely — and a typed client generated from that schema loses the guarantee that makes it worth generating. Four fixed read models are four types a client can name.

## Revisit this decision when

- A fifth and sixth read model appear within one feature. That suggests the entity boundary is drawn in the wrong place, not that another projection is needed.
- A summary needs a field no single query can produce. The projection is then a composition of two reads, and the composition belongs in the API layer rather than in a port.
- The API serves more than one client with different field needs. The `?expand=` objection above is about a typed generated client, and a second client without one changes the balance.

## References

- [ADR-0002: Use SeaORM for persistence](0002-use-seaorm-for-persistence.md) — why the SQL here is not compile-time verified
- [ADR-0009: Hand-write Query hooks with semantic key factories](0009-hand-write-query-hooks-with-semantic-key-factories.md) — the cache keys these endpoints are addressed by
- [PostgreSQL: expression evaluation order](https://www.postgresql.org/docs/current/sql-expressions.html#SYNTAX-EXPRESS-EVAL) — the guarantee that does not exist, behind the callout above
- [RFC 9110 §8.3](https://www.rfc-editor.org/rfc/rfc9110#section-8.3) — content negotiation, for why `?expand=` is not the media-type mechanism it resembles
