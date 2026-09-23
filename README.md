# Nyuka

A self-hosted manga library: installs third-party source extensions, browses
and searches them, downloads chapters, and packages them as CBZ files that
other readers can open.

One Rust binary serves the REST API, the SSE event stream, and the embedded
React application from a single origin.

## Architecture

The decisions, and the reasoning behind them, are in
**[`docs/adr/`](docs/adr/README.md)** — start with that index. The longer design
documents are [`backend-architecture.md`](backend-architecture.md),
[`frontend-architecure.md`](frontend-architecure.md), and
[`spec-design.md`](spec-design.md).

```
crates/
  domain/           entities, value objects, ports (traits) — no I/O
  persistence/      SeaORM entities and repository adapters
  jobs/             queue adapter, worker pool, scheduler, handlers
  aidoku-runtime/   wasmtime host, .aix loader, host imports
  packaging/        CBZ writer, ComicInfo.xml, library layout
  api/              Axum routes, OIDC, SSE, static embed; main.rs
web/                React application (Vite), embedded at build time
```

Everything points inward to `domain`, which has no I/O dependencies. `api` is
the composition root. CI enforces the dependency rule.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.98 | Pinned in `rust-toolchain.toml`. The floor is 1.96, set by `vergen` 10 — not by edition 2024 |
| Node | ≥ 22.12 | Vite 8 baseline |
| PostgreSQL | 17 | Via Compose or locally |
| Docker | — | For the Compose stack |

## Setup

```bash
cp .env.example .env          # then fill in the OIDC and session secrets
docker compose up -d postgres flaresolverr
cargo run -p nyuka-api        # API on :8080

cd web && npm install && npm run dev   # Vite dev server, proxying /api
```

Migrations run on boot. `/readyz` reports whether they are current.

For local development the frontend runs from the Vite dev server rather than
the embedded copy — a release build recompiles the Rust crate on every asset
change, which is a poor edit loop.

## Configuration

Every variable is documented in [`.env.example`](.env.example). Configuration
is validated at startup and the process exits on anything missing or
malformed, rather than failing at first use.

Secrets — `OIDC_CLIENT_SECRET`, `DATABASE_URL`, `SESSION_KEY` — arrive from
Docker secrets or a vault. They are never baked into an image, and a
redaction layer keeps them out of logs and spans.

## Runbook

### Production needs HTTP/2

Run behind the `caddy` service with TLS. This is not only about encryption: an
SSE stream occupies one of the browser's six HTTP/1.1 connections per origin,
and once they are gone **the seventh request of any kind queues with no error
and no timeout**. The symptom is the whole application appearing to freeze
after a user opens roughly six tabs, with nothing in the logs. Browsers do not
speak h2c, so TLS is what gets you HTTP/2. See ADR-0010.

### Debugging a failure

Logs are JSON on stdout, and every line carries `trace_id`:

```bash
docker logs nyuka-api | jq 'select(.span.trace_id=="<id>")'   # one causal chain
docker logs nyuka-api | jq 'select(.level=="ERROR")'
docker logs nyuka-api | jq 'select(.target=="nyuka_metrics")' | tail -1
```

The trace ID shown on the application's crash page is the one to grep, and it
also comes back on the `x-trace-id` response header of every request. A
browser interaction, the API request it caused, and the job that ran minutes
later all share it.

### Evaluating the objectives

The two SLOs are defined in [ADR-0019](docs/adr/0019-define-the-two-service-level-objectives.md)
and evaluated from the hourly `nyuka_metrics` line, because a monthly window
outlives the log rotation.

```bash
# SLO-1 — service-attributable job success rate over the retained window.
# Source faults are excluded on purpose: a site removing a chapter is not a
# failure this service can act on.
docker logs nyuka-api \
  | jq -c 'select(.target=="nyuka_metrics") | .metrics | fromjson' \
  | jq -s '{
      succeeded:      (map(.jobs_succeeded)      | add),
      failed_service: (map(.jobs_failed_service) | add),
      failed_source:  (map(.jobs_failed_source)  | add)
    } | . + {
      rate: (.succeeded / ((.succeeded + .failed_service) | if . == 0 then 1 else . end))
    }'

# SLO-2 — the worst hourly p95 in the window. Target: 60000 ms.
# `ttfp_truncated` matters: a true there means that hour's p95 is a lower
# bound, not a measurement.
docker logs nyuka-api \
  | jq -c 'select(.target=="nyuka_metrics") | .metrics | fromjson' \
  | jq -s 'map(select(.ttfp_samples > 0))
           | max_by(.ttfp_p95_ms)
           | {ttfp_p95_ms, ttfp_samples, ttfp_truncated}'
```

An hour with no attributable outcomes reports no rate rather than a perfect
one — an idle hour is not a good hour, and averaging a fabricated 100% across
a month would hide a real dip.

### Backups: the library and the database fail differently

`pg_dump` covers metadata. It does **not** cover the library volume, so a disk
failure loses downloaded files while the database still references them. Back
up `/library` separately.

```bash
docker compose exec postgres pg_dump -U nyuka nyuka | gzip > nyuka-$(date -I).sql.gz
tar -C /library -czf library-$(date -I).tar.gz .
```

The two do not need to be consistent with each other. A library newer than the
database has files nothing references, which costs disk and nothing else. A
database newer than the library references files that are gone, which
`reconcile_library` repairs.

#### Restoring

```bash
gunzip -c nyuka-2026-09-22.sql.gz | docker compose exec -T postgres psql -U nyuka nyuka
tar -C /library -xzf library-2026-09-22.tar.gz
docker compose restart nyuka-api
```

Then reconcile, so the database stops claiming chapters the restore did not
bring back:

```bash
curl -si -X POST http://localhost:8080/api/v1/jobs \
  -H 'content-type: application/json' \
  -H 'x-requested-with: XMLHttpRequest' \
  -b "$SESSION_COOKIE" \
  -d '{"kind":"reconcile_library"}'
# 202 Accepted, with Location naming the job to watch.
```

Only the maintenance kinds can be queued this way. The rest take a payload
naming a chapter or a follow, and each has an endpoint that validates it.

`reconcile_library` clears the download record for any chapter whose file is
missing, so the UI stops offering a read that would fail. It **refuses to run
at all if the library root is unreadable** — an unmounted volume looks exactly
like an emptied one, and treating the first as the second would clear every
download record during a five-minute outage.

> [!WARNING]
> Restore the library **before** starting the service, or start it with the
> volume already mounted. The reconciliation is safe against an unreadable
> root but not against a mounted-and-empty one: an empty volume is
> indistinguishable from a wiped library, and the records will be cleared.

### Disk full

A download that exhausts the volume fails with a distinct error, leaves no
partial file — archives are placed by atomic rename — and `/readyz` reports the
library as not writable. Free space and restart the affected jobs.

### A source stops working

Expected periodically: sites change markup, and Cloudflare challenge handling
is adversarial by nature.

```bash
docker logs nyuka-api | jq 'select(.target=="nyuka_jobs" and .level=="WARN")'
```

If a source needs a host capability this build does not implement,
installation is refused with the capability named — that is a host gap, not a
site problem. `canvas` is the common one: about 16% of the community catalog
uses it to descramble page images, and v1 does not implement it. See
[ADR-0004](docs/adr/0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md).

> [!NOTE]
> `FLARESOLVERR_URL` is read and validated at startup but **nothing uses it
> yet**. Cloudflare challenge handling is not implemented, so a source behind
> one fails as an ordinary source error. `/readyz` reports the database,
> migration state, and the library volume — not FlareSolverr.

### Revoking access

`POST /api/v1/auth/logout` ends one session, immediately — the row is deleted
rather than left to expire.

> [!IMPORTANT]
> **There is a revocation gap, and it is deliberate.** Sessions have their own
> lifetime and are not tied to identity-provider token expiry. If the provider
> disables an account, this application's session stays valid until it expires
> or someone deletes it. `SESSION_TTL_HOURS` bounds the window — it defaults
> to 720, which is 30 days. ADR-0005 accepts this as the cost of not requesting
> `offline_access`; shorten the TTL if the window matters more than the
> sign-in frequency.

To force a logout before then, delete the user's sessions:

```bash
# Find the user. `subject` is their identifier at the identity provider.
docker compose exec postgres psql -U nyuka -c \
  "SELECT id, issuer, subject, last_seen_at FROM app_user ORDER BY last_seen_at DESC;"

# End every session they hold. Takes effect on their next request: expiry is
# enforced on read, so there is no cache to wait out.
docker compose exec postgres psql -U nyuka -c \
  "DELETE FROM session WHERE user_id = '<uuid>';"
```

Deleting the `app_user` row instead does **not** revoke access — it removes
the audit record while the session rows survive, and the next sign-in simply
recreates the user. Delete sessions, not users.

To lock someone out permanently, remove them from `AUTH_ALLOWED_SUBJECTS` or
`AUTH_ALLOWED_GROUPS` and restart. The allow-list is checked at the callback,
so an existing session still has to be deleted as well.

### Rollback

Redeploy the previous image tag. The frontend is embedded in the binary, so a
rollback reverts the API and the UI together — they cannot end up mismatched.

## Development

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check

cd web
npm run check        # Biome: lint + format
npm run typecheck
npm run test
npm run i18n:check   # fails when catalogs are stale
```

Three generated files are committed on purpose, and CI fails when any is
stale: `web/openapi.json`, `web/src/shared/api/schema.d.ts`, and
`web/src/routeTree.gen.ts`.

## Licence

AGPL-3.0-or-later.
