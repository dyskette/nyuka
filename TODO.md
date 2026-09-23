# TODO

Work remaining on nyuka, and the decisions still open. Items trace back to the
ADR that created them, so the reason for each is one link away.

Status as of the last commit: the backend is complete — `domain`,
`persistence`, `aidoku-runtime`, `packaging` and `jobs` have every port
implemented, and `api` serves all 31 operations. The frontend has its shell
and its Library screen; the other screens are stubs that say so.

## Decisions that need an answer

These block work rather than being work. Two need the project owner; one has a
recommendation in its own ADR that can simply be adopted.

- [x] **Define the two SLOs** — done in
      [ADR-0019](docs/adr/0019-define-the-two-service-level-objectives.md).
      99.9% service-attributable job success and 95% of user-requested
      downloads readable within 60s, both monthly, evaluated from an hourly
      `nyuka_metrics` snapshot line. Settles `NOTIFY` as unnecessary — it buys
      at most 2% of the budget — and keeps `vector`/Loki unbuilt.
- [x] **The v1 source commitment** (ADR-0004 follow-up 1). Resolved as
      capability-defined rather than a named list: v1 supports any source
      whose imports are tier 1 (`net`, `std`, `html`, `defaults`) — 108 of 136
      community sources, 79%. The install check already enforces that
      boundary, so the promise and the enforcement are one mechanism. Seven
      proven sources are the regression set; `canvas` (16%, concentrated in
      Japanese and Vietnamese) is deferred and refused at install by name.
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
- [x] **Sources** — list, install, uninstall, filters, and per-source
      settings. Values cross the wire base64-encoded: they are postcard bytes
      the source owns, and decoding would mean guessing at a schema this
      server does not have.
- [x] **Catalog** — browse a source, item details, item chapters
- [x] **Add to library** — `POST /manga` from a catalog entry
- [x] **Downloads** — `POST /downloads` with `Idempotency-Key`, 202 + `Location`
- [x] **Chapter file** — `GET /downloads/{chapter_id}/file`, ranged, ETag from
      the stored checksum
- [x] **Telemetry ingest** — `POST /telemetry` (ADR-0013), both OTLP/JSON
      and OTLP/protobuf. The two decoders converge on one filter, with a
      test that the same batch produces the same result either way: an
      allow-list enforced in one and not the other is the shape of bug
      where a client picks the encoding that skips the check.
- [x] **Trigger a maintenance job by hand** — `POST /jobs`, restricted to
      the four maintenance kinds. The runbook no longer documents a raw
      insert, which was a schema dependency in prose.
- [x] **Embedded SPA** — `rust-embed` over `web/dist`, with the route
      precedence rules (ADR-0006). `build.rs` writes a placeholder
      `index.html` when the directory is absent, so the backend builds and
      tests without a frontend toolchain.
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
- [x] **Property-test `paths.rs`** with traversal sequences, null bytes,
      RTL overrides, Windows reserved names and 300-character names
      (ADR-0007 follow-up 1). The named shapes are an explicit corpus, since
      a generator reaches them only by luck.
- [x] **Property-test the postcard decode path** (ADR-0004 follow-up):
      arbitrary bytes, huge declared lengths, truncated values and single-byte
      flips against every type decoded from guest memory.
- [x] **Property-test the OTLP decode path** (ADR-0013 follow-up 2):
      arbitrary bytes, deep nesting, and a valid envelope with hostile
      contents, asserting the allow-list and limits hold and that whatever
      survives serializes to one line.
- [ ] **Coverage-guided fuzzing** for those same three paths. The property
      tests above are *not* fuzzing: they explore what the generators reach,
      not what the code branches on. `cargo-fuzz` needs nightly for its
      sanitizers and this workspace pins a stable toolchain, so this needs a
      separate nightly job rather than a line in the main CI run.
- [x] **Validate a generated CBZ against a real reader's rules** — the vendored
      ComicInfo v2.0 schema via `xmllint`, plus the filename conventions Komga
      and Kavita document (ADR-0007 follow-up 4). Found a real defect: a
      fractional volume emitted `3.5` into an `xs:int` element and invalidated
      the whole document.
- [x] **Auth negative paths needing a provider** — mismatched `state`, a
      subject absent from the allow-list, a callback with no flow, a replayed
      code, sign-out with and without CSRF, and the rate limit. In
      `web/e2e/auth.spec.ts`.
- [ ] **Three paths a configurable stub cannot reach** — a replayed `nonce`, a
      mismatched PKCE verifier, an expired authorization code. Each needs a
      provider that can be driven into misbehaving rather than configured, so
      closing them means writing one. `crates/api/tests/auth.rs` says so
      precisely rather than implying the stack covered everything.
- [x] **Log-injection resistance** — a span name with newlines, quotes and
      ANSI escapes must serialize to one valid JSON line `jq` parses
      (ADR-0013 follow-up 4).

## Build and release

- [x] **Scheduled live-source run** — `cargo xtask fetch-sources` plus the
      live test, nightly and report-only (ADR-0004). Not a pull-request gate:
      a failure there cannot distinguish a host regression from markup drift,
      which is what the conformance fixture settles.


- [x] **`cargo xtask openapi`** writing `web/openapi.json`, and the CI
      freshness check that fails when it is stale (ADR-0008, ADR-0009)
- [x] **`cargo-deny` exception for RUSTSEC-2023-0071** with the reasoning from
      ADR-0005, an owner, and a review date (`deny.toml`, review by
      2026-12-21). `bans` now denies a second `sqlx`, `opentelemetry`,
      `opentelemetry_sdk` or `wasmtime` outright — that was a comment saying
      it would be a problem, and is a check now.
- [ ] **Pin the OTel crates in one Renovate group** with `tracing-opentelemetry`
      and `axum-tracing-opentelemetry`, so an upgrade is one reviewed change
      (ADR-0014 follow-up 2)
- [ ] **Tune the `job` table** — lower `autovacuum_vacuum_scale_factor`
      (ADR-0003 follow-up 2; the partial index and retention job are done)
- [ ] **Review the 60s time-to-first-page budget** after a month of real
      snapshots (ADR-0019 follow-up 5). It is an estimate until then.
- [x] **Rendered-contrast check** over the built CSS, both themes, focus ring
      included (ADR-0016). Fourteen token pairs measured as painted, plus
      `axe-core` over the rendered shell. Found and fixed a real 2.54:1
      failure. This was the last open load-bearing follow-up.

## Frontend

In progress. ADR-0006 and ADR-0008 through ADR-0018 cover it.

This list tracks screens, and that is not the same as tracking the API. Three
operations were served with nothing reaching them and no entry here saying so,
because every *resource* had a screen — checking
`openapi.json` against `web/src` is what found them, and is worth repeating
before claiming the frontend is complete.

Every item below traces to an ADR, the mockup in `docs/design/`, or a gap in
code that already exists. Nothing goes on this list because it seemed like a
thing a manga server would have — an in-browser reader was listed that way and
has been removed.

**What the mockup is.** A reference for layout, density, spacing and colour —
how the interface is distributed, not what it does. Its screens are the
authority on *how a library table looks*; they are not a feature list. Where a
mockup element implies a capability the API does not have, the capability is a
separate decision and the element is not built until it is made. "Unread 32"
and the "14 new" badges were read as scope once, and were not.

- [x] Vite 8 / Rolldown scaffold, TanStack Router and Query
- [x] Typed API client, problem+json handling, i18n negotiation
- [x] Generated client from `openapi.json`, committed and checked by CI
- [x] Lingui catalogs wired, English and Spanish complete
- [x] App shell — sidebar, status bar, and a route per navigation target
- [x] **Library** — the dense table from the mockup: source, chapter count,
      download progress and freshness per row, multi-select, sortable headers
      and a filter bar. Ordering and filters live in search params; selection
      does not.
- [x] **Browse** — a source's catalog in the cover grid, with search, paging,
      and add-to-library driven by the `manga_id` the server already sets.
- [x] **Downloads** — the job queue with live progress, per-row cancel and
      retry, and a URL-driven state filter. Maintenance jobs appear alongside
      downloads rather than being hidden: a stuck prune is what an operator
      needs to see.
- [x] **Settings** — repositories and the sources installed from them: add,
      refresh, remove, install, update, uninstall.
- [x] **Follows** — the schedule table: missing-chapter count, interval,
      auto-download, check-now and unfollow.
- [x] **Per-source settings** — rendered from the package's own declaration,
      with a postcard codec held to the Rust serializer by printed vectors.
      Five control types cover 95 of the 136 declared across community
      sources; the rest are named as unavailable rather than hidden.
- [x] **Source filters** — `GET /sources/{id}/catalog?filters=<JSON>` reaches
      the WASM module, and Browse renders the declaration. Five control types
      cover 516 of the 534 declared across community sources.
- [ ] **Browse detail panel.** `GET /sources/{id}/catalog/{key}` and its
      `/chapters` are served and unreached: a catalog card offers add-or-open
      and nothing shows a description or a chapter list before adding. The
      mockup has this panel.
- [ ] **Downloads detail panel.** `GET /jobs/{id}` is served and unreached.
      The mockup shows a job's pages, worker, output path and event log; the
      queue row shows state and progress only.
- [ ] **Retrieving a chapter file.** `GET /downloads/{chapter_id}/file` is
      served, ranged and ETagged, and nothing in the UI links to it — a
      chapter can be downloaded to the server and not to the person who asked
      for it. Interoperability (ADR-0007) covers pointing another reader at
      the volume; it does not cover getting one file out.
- [ ] **A `Range` filter value on the host.** 18 of 534 declared filters are
      `range` and `FilterValue` has no variant for one, so the client names
      them as unavailable rather than rendering a control the server would
      refuse. Adding it means a guest-ABI change, so it waits for a source
      that needs it.
- [x] Detail panel (`/library/$mangaId`) — header, URL-driven tabs, and the
      chapter list with a per-row download action
- [x] **Command palette** (`cmdk`), from the mockup — library search,
      navigation, contextual actions on the highlighted series, and the four
      maintenance jobs, which have no other UI. The mockup's "Refresh
      metadata" is left out: the server answers 400 for that kind.
- [x] MSW wired up for component tests, with handlers typed against the
      generated schema and `onUnhandledRequest: 'error'`
- [x] **Virtualized list** (TanStack Virtual) — padding rows rather than
      absolute positioning, so the table stays a table, with `aria-rowcount`
      and `aria-rowindex` carrying the real numbers.
- [x] SSE provider and the reconnect invalidation set — one `EventSource` at
      the root, an invalidation table per event, and invalidate-on-connect for
      both first connections and reconnects. The status bar reads the stream
      rather than always claiming "Live".
- [x] **Playwright stack** with the stub OIDC container — seven auth paths,
      each verified by breaking the control it guards. Three API instances,
      because the sign-in rate limiter is per process.
- [x] **Server-side sort, filter and search** — `GET /manga` takes `q`,
      `status`, `source_id`, `sort` and `dir`, with the keyset built on the
      chosen sort column. The client orders and filters nothing.
- [x] **Infinite scroll** — one `useInfiniteQuery` over the server's cursor,
      fetching the next page as the end comes into view.
- [x] **Read progress** — decided against. There is no reader, nothing records
      what has been read, and the mockup's "Unread 32" and "14 new" were never
      a commitment to track it. Where a count belongs, the screens show what is
      *downloaded*, which the server can defend.

## Documentation

- [x] **Library runbook** (ADR-0007 follow-up 6) — backup, restore, and
      reconciliation, in `README.md`. The restore procedure warns that an
      empty mounted volume is indistinguishable from a wiped library.
- [x] **IdP-revocation gap** in the runbook (ADR-0005 follow-up 7), with the
      SQL for forcing a logout and a note that deleting the `app_user` row
      revokes nothing.
- [x] **`jq` recipes** in the runbook (ADR-0014 follow-up 5), including the
      monthly evaluation for both objectives from ADR-0019.
- [x] **README and CHANGELOG** — README corrected against the code (it
      claimed `/readyz` reports FlareSolverr reachability and referenced a
      span that does not exist); CHANGELOG brought up to date.

## Known gaps recorded in code

These are deliberate, documented, and not scheduled. Listed so they are not
rediscovered as bugs.

- **`JobEvent::SourceUpdated` is never published.** The SSE layer names it and
  the client listens for it; nothing emits it. The sibling `JobState` had the
  same shape of bug and was only found by driving the real stream with `curl`
  — a variant existing is not a variant being sent. It belongs in the source
  refresh path, and the Sources screen is what will need it.

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
