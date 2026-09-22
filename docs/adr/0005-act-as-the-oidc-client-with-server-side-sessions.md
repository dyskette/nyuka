# ADR-0005: Act as the OIDC client, with server-side sessions

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/api`, `crates/persistence` |
| **Supersedes** | None |

This article explains why the API is itself a confidential OIDC client that runs the authorization-code flow and issues a session cookie, rather than delegating authentication to a reverse proxy or validating bearer tokens minted for the browser. It also records two implementation constraints the dependency graph forces on you: you must write the session store, and you must make a documented decision about an unpatched advisory in the OIDC dependency tree.

## Context

The deployer supplies their own identity provider — Authelia, Keycloak, or Google are the expected cases — and decides which subjects or groups may sign in. The API must authenticate browser users against that provider, keep a user record for identity and audit, and protect every endpoint under `/api/v1`.

### The browser cannot attach headers to three of the endpoints

This is the constraint that decides the shape of the solution, and it follows from decisions already made.

| Endpoint | How the browser calls it | Can it send `Authorization`? |
|---|---|---|
| `GET /api/v1/events` | `EventSource` | **No.** The `EventSource` API has no facility for request headers. |
| `GET /api/v1/downloads/{id}/file` | Navigation or anchor download | **No.** |
| Cover images | `<img src>` | **No.** |

Per [ADR-0001](0001-use-axum-for-the-http-api.md) and [ADR-0003](0003-run-the-job-queue-in-postgres-and-in-process.md), SSE is the only live-state channel in the product; the frontend has no polling fallback. Any authentication scheme that depends on setting a request header cannot cover it. The workarounds are a token in the query string, which lands in access logs and in the `url.path` span attribute, or a cookie. Cookies are sent automatically by all three call shapes.

### The SPA is same-origin

The React application is embedded in the binary with `rust-embed` and served from the same origin as the API. There is no cross-origin boundary, so there is no CORS requirement and no need for the browser to hold a credential it can present to a *different* host. The browser only ever talks to this server.

The flip side is that `SameSite` cookies alone do not protect state-changing requests, so CSRF protection is still required.

### Other constraints

- **Access is an allow-list, not a role model.** Config names the permitted OIDC subjects or groups. The library itself is shared: `user` exists for identity and audit, not for per-user data partitioning.
- **Tests must run without a proxy.** The Playwright suite runs the real binary against a seeded Postgres and a stub OIDC provider container. Whatever authenticates in production must authenticate in that stack.
- **Persistence is SeaORM 2.x on SQLx 0.9**, per [ADR-0002](0002-use-seaorm-for-persistence.md). Anything that needs a database connection should use that pool.
- **Deployment is one Compose host** with an optional `caddy` for TLS. A reverse proxy is optional today, which means the API cannot depend on one being present.

## Decision

The API is a **confidential OIDC client** implementing the backend-for-frontend (BFF) pattern.

- **Flow.** Authorization code with PKCE, plus `state` and `nonce`, via `openidconnect` 4.0.1. Tokens from the IdP never reach the browser.
- **Session.** Server-side session stored in PostgreSQL through `tower-sessions` 0.15.x. The browser receives only an opaque session cookie: `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`.
- **Session store.** Implement `SessionStore` directly against SeaORM. Do not add an off-the-shelf store crate; see [You must write the session store](#you-must-write-the-session-store).
- **Authorization.** Check the configured subject or group allow-list at the callback, before creating a session. Fail closed: an empty or missing allow-list denies everyone rather than admitting everyone.
- **CSRF.** Require the `X-Requested-With` header on every state-changing method in middleware, not per handler. `SameSite=Lax` plus a header a cross-site form cannot set is sufficient here because the API is same-origin and accepts only JSON bodies.
- **Session lifetime is the application's own.** Do not tie it to IdP token expiry. Store a refresh token server-side only if `GET /me` needs to re-read claims from the IdP; otherwise do not request `offline_access` at all.
- **Expired-session cleanup is a job kind**, reusing the scheduler from ADR-0003 rather than a bespoke timer.

> [!IMPORTANT]
> If a reverse proxy is later placed in front of the API, the API must still not trust identity headers from it. `TRUSTED_PROXIES` governs `X-Forwarded-*` for client-address purposes only. Authentication stays the API's own decision, because an application that trusts `X-Forwarded-User` is fully compromised the moment it becomes reachable by any other route.

This matches the architecture that [RFC 10017, *OAuth 2.0 for Browser-Based Apps*](https://oauth.net/2/browser-based-apps/) identifies as the most secure of the three it describes: a BFF holding tokens server-side and communicating with the browser through `HttpOnly` cookies. The decision is therefore aligned with a published standard rather than merely conventional.

### You must write the session store

Every published `tower-sessions` store that could hold sessions in Postgres conflicts with the versions this project already uses. This was verified against crates.io metadata, not inferred:

| Store crate | Latest | Blocker |
|---|---|---|
| `tower-sessions-sqlx-store` | 0.15.0, published Jan 2025 | Requires `sqlx ^0.8.0`, semver-incompatible with SeaORM 2.0's `sqlx 0.9`. Separately, it pins `tower-sessions-core ^0.14.0` while `tower-sessions` 0.15.0 pins `tower-sessions-core =0.15.0`, so it does not pair with the current `tower-sessions` either. |
| `tower-sessions-seaorm-store` | 0.1.1, published Jul 2025 | Requires `sea-orm ^1.1.11` and `tower-sessions ^0.14.0`. Both conflict. |

Adopting either would mean a second SQLx version and a second connection pool in the process, which is the same trap identified for job-queue crates in ADR-0003.

Writing it is small. `SessionStore` is `Debug + Send + Sync + 'static` with three required methods — `load(&Id)`, `save(&Record)`, `delete(&Id)` — and one provided method, `create(&mut Record)`. Against SeaORM that is an entity, a migration, and roughly a hundred lines, on the pool that already exists. Record serialization uses `rmp-serde` in the published stores; match that or use JSON, and write the choice down, because changing it later invalidates every live session.

### The `rsa` advisory must be handled deliberately, not silenced

`openidconnect` 4.0.1 depends on `oauth2 ^5.0.0`, `rsa ^0.9.2`, and `p256 ^0.13.2`. The `rsa` crate carries [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), the Marvin attack: a non-constant-time implementation leaking private-key information through network-observable timing. As of 12 September 2026 it is **still unpatched**, affecting both `rsa` 0.9.10 and `rsa` 0.10.0-rc.18, with constant-time work in progress upstream.

The organizational standard puts `cargo-deny` and `cargo-audit` in the pipeline, so this will fail the build on the first commit that adds `openidconnect`. Do not resolve that with an unexplained ignore. The applicable analysis is:

- The advisory concerns **RSA private-key operations**. A relying party verifying an `RS256` ID token uses the provider's **public** key. Signature verification does not exercise the vulnerable path.
- Two configurations would exercise it: **`private_key_jwt` client authentication**, which signs with your RSA private key, and **encrypted ID tokens** (JWE with RSA key wrapping), which decrypt with your RSA private key.

So the decision is: use `client_secret_basic` or `client_secret_post` for client authentication, do not enable encrypted ID tokens, prefer an IdP signing algorithm of `ES256` where the provider supports it, and record a `cargo-deny` exception for RUSTSEC-2023-0071 that states this reasoning inline, names an owner, and carries a review date. An advisory exception with a written rationale is a security control. An advisory exception without one is a hole with a comment next to it.

## Consequences

### What you gain

- **Tokens never enter the browser.** An XSS payload cannot read an `HttpOnly` cookie, and there is no access token or refresh token in `localStorage`, `sessionStorage`, or JavaScript memory to exfiltrate. The blast radius of a successful script injection drops from "attacker holds durable credentials for the IdP-issued token lifetime" to "attacker can act through the victim's browser while the session lives".
- **SSE, file downloads, and images authenticate with no special handling.** The cookie is sent by `EventSource`, by an anchor download, and by `<img>`. This is the constraint that made the alternatives expensive and it disappears entirely.
- **Session lifetime, revocation, and logout are yours.** `POST /auth/logout` deletes a row. There is no waiting for a token to expire and no token-revocation endpoint to call.
- **Authorization is enforced where the data is.** The subject and group allow-list runs in the callback with the config the application validated at startup, in the same process that owns the library.
- **The test stack stays simple.** A stub OIDC provider container plus the binary is the whole dependency. No proxy in the loop for Playwright runs or local development.
- **Identity appears in telemetry safely.** The observability design records `user.id` and `session.id` as hashes on the server span, which is only possible because the server resolves identity itself.

### What it costs you

- **You own the flow.** PKCE verifier storage, `state` and `nonce` validation, callback error handling, discovery-document and JWKS caching with refresh, and clock-skew tolerance are all application code. Each is a place where a subtle mistake is a real vulnerability, and none of it is exercised by ordinary feature work, so it needs its own tests.
- **You own the session store** and its serialization format, expiry semantics, and cleanup job.
- **`openidconnect` 4.0.1 is not recently maintained.** Published July 2025, roughly fourteen months old at the time of writing, and it pulls `base64 ^0.21` while the ecosystem has moved to 0.22. It is the standard choice and it works, but budget attention for it rather than assuming upstream will move.
- **A documented advisory exception exists in the build.** It must be reviewed on a schedule, and the review must actually happen.
- **No single logout and no IdP session coupling.** If the IdP revokes or disables an account, this application's session remains valid until it expires or is deleted. With an application-controlled session lifetime this is a bounded window, but it is a real gap: state it in the runbook rather than discovering it during an incident.
- **CSRF protection is a middleware invariant that must not regress.** A future endpoint that accepts a form-encoded body, or a handler mounted outside the middleware stack, reopens the hole. Cover it with a test that asserts a mutation without `X-Requested-With` is rejected.

### Follow-up work this decision creates

1. **Implement `SessionStore` against SeaORM** with an entity, a migration, and a documented record-serialization format.
2. **Add the expired-session cleanup job kind** to the scheduler from ADR-0003.
3. **Write the `cargo-deny` exception** for RUSTSEC-2023-0071 with the reasoning above, an owner, and a review date, and configure the client to use `client_secret_*` authentication with encrypted ID tokens disabled.
4. **Test the negative paths**, not just the happy path: mismatched `state`, replayed `nonce`, missing PKCE verifier, expired authorization code, a subject absent from the allow-list, an empty allow-list denying all, and a mutation missing `X-Requested-With`.
5. **Validate auth configuration at startup.** Issuer URL reachable, client ID and secret present, redirect URI matching what is registered, allow-list non-empty. Fail fast rather than at first login.
6. **Rate-limit `/auth/*`** with `tower_governor`, as already specified, and make sure the limit applies to the callback as well as the login initiation.
7. **Document the IdP-revocation gap** in the runbook, with the operator action for forcing a logout.

## Alternatives considered

| Alternative | Where identity is decided | Outcome |
|---|---|---|
| Backend as OIDC client, server-side session | In the API | **Chosen.** RFC 10017's most secure pattern. |
| Reverse-proxy forward-auth | In the proxy | Rejected. Trust model depends on network reachability; complicates dev and tests. |
| Bearer tokens validated per request | In the API, credential in the browser | Rejected. Cannot authenticate `EventSource`, downloads, or images. |
| Token-mediating backend | Split | Rejected. Keeps tokens in the browser for no benefit at same origin. |
| Local accounts with passwords | In the API | Rejected. Reimplements what the deployer's IdP already provides. |

### Reverse-proxy forward-auth

This is the strongest alternative. Authelia, `oauth2-proxy`, Traefik `forwardAuth`, or Caddy's auth portal terminate authentication ahead of the application, and the application reads an identity header. The API would contain no OIDC code at all — no PKCE, no JWKS caching, no session store, and no `rsa` advisory in its tree. For a self-hosted deployment where the operator already runs such a proxy, that is a genuinely attractive reduction in scope, and it is the conventional answer in this ecosystem.

It was rejected on four grounds:

1. **The trust model is network reachability.** Identity arrives as a header the application must believe. That is only safe if the application is unreachable by any other path. On a single Compose host this is arranged with network configuration rather than enforced by the application, and it fails silently: exposing a port for debugging, or another container resolving the service name, turns a header into an authentication bypass. The failure is total and leaves no trace in application logs.
2. **It does not remove the session.** The API still needs a `user` row for audit and a session for CSRF state and request correlation, so the proxy replaces the OIDC flow but not the session layer — perhaps sixty percent of the work, for the trust problem above.
3. **It enters the test stack.** Playwright runs against the real binary. Forward-auth means the proxy and its configuration become part of every end-to-end run and every developer's local setup, replacing a single stub-OIDC container.
4. **Allow-list granularity moves out of the application.** Deciding which subjects may sign in becomes proxy configuration, so the application cannot enforce or report on its own access policy, and `/readyz` cannot validate it.

If a proxy is introduced later for other services, this decision does not need to change: the API keeps authenticating itself, and the proxy handles TLS and routing.

### Bearer tokens validated per request

The resource-server model — the SPA runs the code flow itself, holds the access token, and sends `Authorization: Bearer` — is what `jsonwebtoken` 11.x plus JWKS caching would implement.

It fails on the header constraint. `EventSource` cannot set `Authorization`, and neither can a download navigation or an `<img>` tag. Covering the live-update channel would require a token in the query string, which would be written to access logs and to the `url.path` span attribute, or an `EventSource` polyfill in the frontend, or a cookie — at which point the cookie is doing the work and the tokens add only exposure.

Independently, RFC 10017 directs browser apps away from holding tokens, recommending memory-only storage at best and the BFF pattern as superior. Choosing this pattern would mean accepting a weaker posture *and* more frontend complexity to solve a problem the chosen design does not have.

### Token-mediating backend

RFC 10017's middle pattern: the backend holds the refresh token and hands short-lived access tokens to the browser. It is a reasonable design when the SPA must call a resource server on a different origin. Here the SPA calls only this server, at the same origin, so handing it a token buys nothing and costs the browser-storage exposure the BFF pattern removes.

### Local accounts with passwords

Rejected. The deployer already operates an identity provider, and taking on password storage, reset flows, MFA, and lockout policy would be a larger and riskier surface than the OIDC client, for less capability. Delegating to the IdP is also what least-privilege identity management calls for.

## Revisit this decision when

- **RUSTSEC-2023-0071 is patched.** Drop the `cargo-deny` exception and remove the constraint on client-authentication method.
- **A second client origin appears**, such as a native mobile app. That client cannot use a same-origin cookie, and a token-based path becomes necessary alongside this one — evaluate the token-mediating pattern and sender-constrained tokens at that point, and do not weaken the browser path to accommodate it.
- **Per-user libraries are introduced.** The allow-list is an authentication gate, not an authorization model. Multi-user data partitioning requires a real permission model and a review of every query in `crates/persistence`.
- **IdP-driven revocation becomes a requirement** rather than a documented gap — for example, if account disabling must take effect immediately. That means back-channel logout or token introspection on each request, and it changes the session design.
- **`openidconnect` stops being maintained.** The flow is implementable directly on `oauth2` 5.x plus a JOSE library, but that is a significant increase in code you own, and it should be a deliberate move rather than a drift.

## References

- [RFC 10017, OAuth 2.0 for Browser-Based Apps](https://oauth.net/2/browser-based-apps/)
- [RFC 7636, PKCE](https://datatracker.ietf.org/doc/html/rfc7636)
- [`openidconnect` on crates.io](https://crates.io/crates/openidconnect)
- [`tower-sessions`](https://github.com/maxcountryman/tower-sessions) and [`SessionStore`](https://docs.rs/tower-sessions/latest/tower_sessions/trait.SessionStore.html)
- [`tower-sessions-sqlx-store` dependency metadata](https://crates.io/crates/tower-sessions-sqlx-store)
- [`tower-sessions-seaorm-store` dependency metadata](https://crates.io/crates/tower-sessions-seaorm-store)
- [RUSTSEC-2023-0071: Marvin attack in the `rsa` crate](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
- [RustCrypto advisory GHSA-c38w-74pg-36hr](https://github.com/RustCrypto/RSA/security/advisories/GHSA-c38w-74pg-36hr)
