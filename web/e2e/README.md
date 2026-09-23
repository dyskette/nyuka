# End-to-end tests

The OIDC paths that need a real token exchange, and therefore a real provider.
[ADR-0005](../../docs/adr/0005-act-as-the-oidc-client-with-server-side-sessions.md)
lists them; `crates/api/tests/auth.rs` covers the ones reachable without one
and names what is still uncovered anywhere.

## Running them

```bash
podman compose -f e2e/compose.yaml up -d   # or docker compose
export DATABASE_URL=postgres://nyuka:nyuka@localhost:5432/nyuka
npm run build                              # the API serves this
cargo build -p nyuka-api
npx playwright install chromium            # first time only
npm run test:e2e
```

Playwright starts the API instances; the stub provider and PostgreSQL are not
started for you, because both are shared with the rest of the suite and
starting them per run would fight whatever else is using them.

## Three servers

| Port | Allow-list | For |
|---|---|---|
| 18080 | `tester` | Signing in, and the callback's refusals |
| 18081 | `somebody-else` | The allow-list denial, and the rate limit |
| 18082 | `tester` | Session lifecycle |

They exist because `/auth/login` and `/auth/callback` share one rate limiter
with a burst of ten (ADR-0005 follow-up 6), and the limiter is per process. A
full sign-in costs two, so eight tests do not fit in one server's budget. The
alternative — arithmetic keeping the whole suite under ten — breaks the moment
anyone adds a test.

Servers are never reused between runs, for the same reason: the bucket would
carry over.

## Two things the stub needs configuring for

Both are in `compose.yaml`, and both cost time to find.

`SERVER_HOSTNAME` sets the **bind** address inside the container. Pinning it to
`127.0.0.1` publishes the port and leaves nothing answering on it. The issuer
the stub advertises already follows the Host header, so it is not needed.

`requestMappings` match the **token** request's form body. `openidconnect`
authenticates with HTTP Basic, so `client_id` is in the `Authorization` header
and not in the body — a mapping on `client_id` never fires. An unmatched
callback does not fall back to sensible claims: it emits a token with no `sub`,
which surfaces as `Failed to parse server response` from a token endpoint whose
response is perfectly well-formed JSON. Matching on `grant_type` works because
it is always in the body.
