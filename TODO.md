# TODO

Work remaining on nyuka, and the decisions still open. Items trace back to the
ADR that created them, so the reason for each is one link away.

Status as of the last commit: the backend is complete below the HTTP layer —
`domain`, `persistence`, `aidoku-runtime`, `packaging` and `jobs` all have
every port implemented. `api` serves health, auth, SSE, and the library,
follows and jobs routes.

## Decisions that need an answer

These block work rather than being work. Two need the project owner; one has a
recommendation in its own ADR that can simply be adopted.

- [ ] **Define the two SLOs** — job success rate and time-to-first-page
      (ADR-0003 follow-up 5, ADR-0014 follow-up 1). Time-to-first-page is the
      input that decides whether Postgres `NOTIFY` is ever needed. If either
      needs window-based evaluation, that is evidence for the `vector`/Loki
      step sooner rather than later.
- [ ] **Pick the target `.aix` sources for v1** (ADR-0004 follow-up 1). Their
      required imports, not the full ABI, define tier-1 and tier-2 scope.
      ADR-0004 calls this the single highest-leverage step in the project and
      it is still open.
- [x] **Cancellation semantics** (ADR-0003 follow-up 6, ADR-0007 follow-up 5).
      Resolved by adopting ADR-0007's own recommendation: delete the staging
      directory on cancel, treat resume as a future feature. Already true in
      practice — atomic rename means nothing partial is ever visible, and
      `clean_staging` at startup is the whole recovery story. Recorded in
      `crates/jobs/src/handlers/download.rs`.

## API surface

- [x] Health probes, `/healthz` and `/readyz`
- [x] OIDC login, callback, logout, `/me`
- [x] Session store over Postgres
- [x] CSRF middleware invariant
- [x] SSE `/events`, with the compression exclusion
- [x] Library routes — `/manga`, `/manga/{id}`, `/manga/{id}/chapters`, `/chapters/{id}`
- [x] Follows — list, get, upsert, delete, check-now
- [x] Jobs — list, get, cancel, retry
- [x] OpenAPI document served at `/api/v1/openapi.json`
- [x] **Source repositories** — CRUD, `POST /{id}/refresh`, `GET /{id}/available`
- [x] **Sources** — list, install, uninstall, filters
      (per-source settings still to do: the `defaults` key-value surface)
- [x] **Catalog** — browse a source, item details, item chapters
- [x] **Add to library** — `POST /manga` from a catalog entry
- [x] **Downloads** — `POST /downloads` with `Idempotency-Key`, 202 + `Location`
- [x] **Chapter file** — `GET /downloads/{chapter_id}/file`, ranged, ETag from
      the stored checksum
- [x] **Telemetry ingest** — `POST /telemetry` (ADR-0013), OTLP/JSON only.
      Protobuf is refused by content type with 415 rather than fed to a JSON
      parser, so whoever hits it is not sent looking in the wrong place.
- [ ] **OTLP/protobuf ingest** — the other half of ADR-0013's accepted
      encodings. The browser SDK can be configured for JSON, so this is a
      convenience rather than a blocker.
- [ ] **Embedded SPA** — `rust-embed` over `web/dist`, with the route
      precedence rules (ADR-0006)
- [x] **Rate limiting** with `tower_governor`, covering `/auth/callback` as
      well as `/auth/login` (ADR-0005 follow-up 6). Uses a custom key
      extractor: the shipped `SmartIpKeyExtractor` trusts `X-Forwarded-For`
      unconditionally, so anyone could opt out of the limit by forging it.
- [x] **Request tracing layer** — `axum-tracing-opentelemetry`, so `trace_id`
      reaches every request line

## Verification owed

Named separately because each is a control that a code review cannot stand in
for.

- [x] **`trace_id` on every line, end to end** — a request and the job it
      enqueues (ADR-0014 follow-up 4). The test found two real bugs: the
      global propagator was never installed, so an incoming `traceparent` was
      silently ignored; and `trace_id` was declared on the server span but
      never recorded, so no line carried one.
- [ ] **Fuzz `paths.rs`** with traversal sequences, null bytes, overlong
      UTF-8, RTL overrides, and 300-character names (ADR-0007 follow-up 1).
      These strings come from untrusted extensions.
- [ ] **Fuzz the postcard decode path** in the runtime (ADR-0004 follow-up).
- [ ] **Fuzz the OTLP decode path** (ADR-0013 follow-up 2) — attacker-
      influenced bytes from an authenticated but untrusted client. Unit tests
      cover non-JSON, empty, truncated, invalid UTF-8 and 2000-deep nesting;
      a fuzzer is still owed.
- [ ] **Validate a generated CBZ against a real reader's rules** — the
      ComicInfo v2.0 schema plus the filename conventions Komga and Kavita
      document (ADR-0007 follow-up 4).
- [ ] **Auth negative paths needing a provider** — mismatched `state`,
      replayed `nonce`, bad PKCE verifier, expired code, a subject absent from
      the allow-list. These belong to the Playwright stack against a stub OIDC
      container; `crates/api/tests/auth.rs` records why they are not unit
      tests.
- [x] **Log-injection resistance** — a span name with newlines, quotes and
      ANSI escapes must serialize to one valid JSON line `jq` parses
      (ADR-0013 follow-up 4).

## Build and release

- [ ] **`cargo xtask openapi`** writing `web/openapi.json`, and the CI
      freshness check that fails when it is stale (ADR-0008, ADR-0009)
- [ ] **`cargo-deny` exception for RUSTSEC-2023-0071** with the reasoning from
      ADR-0005, an owner, and a review date. An advisory exception with a
      written rationale is a security control; one without is a hole with a
      comment next to it.
- [ ] **Pin the OTel crates in one Renovate group** with `tracing-opentelemetry`
      and `axum-tracing-opentelemetry`, so an upgrade is one reviewed change
      (ADR-0014 follow-up 2)
- [ ] **Tune the `job` table** — lower `autovacuum_vacuum_scale_factor`
      (ADR-0003 follow-up 2; the partial index and retention job are done)

## Frontend

Not started. ADR-0006 and ADR-0008 through ADR-0018 cover it.

- [ ] Vite 8 / Rolldown scaffold, TanStack Router and Query
- [ ] Generated client from `openapi.json`
- [ ] The reader
- [ ] Lingui catalogs
- [ ] Playwright stack with a stub OIDC container

## Documentation

- [ ] **Library runbook** (ADR-0007 follow-up 6) — what to back up, how to
      restore, how to recover when the database references a missing file
- [ ] **IdP-revocation gap** in the runbook (ADR-0005 follow-up 7), with the
      operator action for forcing a logout. `delete_for_user` exists; the
      procedure is not written down.
- [ ] **`jq` recipes** in the runbook (ADR-0014 follow-up 5) — follow one
      trace, list failed jobs in the last hour, find slow WASM calls
- [ ] **README and CHANGELOG** — purpose, setup, env vars, runbook links

## Known gaps recorded in code

These are deliberate, documented, and not scheduled. Listed so they are not
rediscovered as bugs.

- `package_chapter` has no handler. Packaging happens inside
  `download_chapter` because `write_chapter` takes page bytes in memory; a
  separate step would need an intermediate raw-page format. See
  `crates/jobs/src/lib.rs`.
- `Last-Event-ID` replay is not implemented. Correctness comes from
  invalidate-on-reconnect, and no `id:` is emitted so the browser never asks
  for a replay this server cannot perform. See `crates/api/src/sse.rs`.
- The OIDC client uses reqwest 0.12 via `oauth2`, so the egress allow-list
  does not cover provider calls. Acceptable because the issuer URL is operator
  configuration, not attacker input. See `crates/api/src/auth/mod.rs`.
- A source's declared rate limit is unknown until its module first runs, so
  `install` records `None` and `upsert_source` coalesces rather than
  overwrites. See `crates/aidoku-runtime/src/adapter.rs`.
