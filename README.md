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
docker logs nyuka-api | jq 'select(.trace_id=="<id>")'     # one causal chain
docker logs nyuka-api | jq 'select(.target=="metrics")' | tail -1
docker logs nyuka-api | jq 'select(.level=="ERROR")'
```

The trace ID shown on the application's crash page is the one to grep. A
browser interaction, the API request it caused, and the job that ran minutes
later all share it.

### Backups: the library and the database fail differently

`pg_dump` covers metadata. It does **not** cover the library volume, so a disk
failure loses downloaded files while the database still references them. Back
up `/library` separately.

After restoring a partial library, the `reconcile_library` job marks rows whose
files are missing so the UI stops offering reads that will fail.

### Disk full

A download that exhausts the volume fails with a distinct error, leaves no
partial file — archives are placed by atomic rename — and `/readyz` reports the
library as not writable. Free space and restart the affected jobs.

### A source stops working

Expected periodically: sites change markup, and Cloudflare challenge handling
is adversarial by nature. Check `/readyz` for FlareSolverr reachability, then
look for `nyuka.cf.challenge` spans. If a source needs a host capability this
build does not implement, installation is refused with the capability named —
that is a host gap, not a site problem.

### Revoking access

`POST /api/v1/auth/logout` ends one session. Note the gap: if the identity
provider disables an account, this application's session stays valid until it
expires or is deleted, because sessions have their own lifetime. To force a
logout, delete the session rows. See ADR-0005.

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
