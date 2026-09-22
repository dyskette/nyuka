# ADR-0006: Embed the frontend in the binary, on one origin

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/api`, `web/` |
| **Supersedes** | None |

This article explains why the compiled React application is embedded in the Rust binary and served from the same origin as the API, rather than shipped as a separate static site or served from a mounted volume.

## Context

The product is a self-hosted application that one operator installs on one machine. Two earlier decisions constrain how the frontend is delivered.

### Same origin is a requirement, not a convenience

[ADR-0005](0005-act-as-the-oidc-client-with-server-side-sessions.md) authenticates the browser with an `HttpOnly; Secure; SameSite=Lax` session cookie, because `EventSource`, file downloads, and `<img>` tags cannot attach an `Authorization` header. That cookie model works cleanly only when the document and the API share an origin. A cross-origin frontend would need `SameSite=None`, credentialed CORS with an explicit origin allow-list, preflight handling on every mutation, and `withCredentials` on the `EventSource`. Each of those is a place to get wrong, and none of them buys anything here.

### The deployment is one container, operated by one person

The runtime target is a Compose host with `api`, `postgres:17`, `flaresolverr`, and optionally `caddy`. Every additional component is something the operator must configure, health-check, upgrade, and back up. The frontend has no server-side rendering requirement and no need to scale independently of the API.

### Version skew has no upside here

The API and the SPA are developed and released together. The TypeScript client is generated from `web/openapi.json`, which is produced by `cargo xtask openapi`, and CI already fails when the two drift. A frontend build that can be deployed independently of the binary that generated its contract is a way to reintroduce skew that the codegen check exists to prevent.

## Decision

Embed `web/dist` in the binary with **`rust-embed` 8.12.x**, using its `axum` feature, and serve it from the same Axum router as the API.

- **Build.** A multi-stage Dockerfile builds `web/` with Node, then builds the Rust workspace with the assets present so `rust-embed` compiles them in. The release binary is the only artifact.
- **Routing.** `/api/v1/*` is matched first. Static assets are served by exact path. Any remaining `GET` that accepts `text/html` returns `index.html` so client-side routing works on deep links and refreshes.
- **API routes never fall back.** An unmatched path under `/api/v1/` returns a problem+json `404`. It must not return `index.html`, because a typo in a fetch call would otherwise surface as an HTML parse error instead of a clear 404.
- **Caching.** Vite emits content-hashed asset filenames, so serve those with `Cache-Control: public, max-age=31536000, immutable`, and serve `index.html` with `no-cache`. Emit `ETag` for embedded assets and honor `If-None-Match`.
- **Content types** come from `mime_guess`, which `rust-embed` already integrates.
- **Development does not use the embedded copy.** Vite's dev server runs on its own port and proxies `/api` to `http://localhost:8080` with `changeOrigin: false` so the session cookie works. `rust-embed` reads from disk in debug builds, so a debug binary picks up a rebuilt `dist` without recompiling.
- **Security headers** are applied by a Tower layer on the document response: a Content-Security-Policy, `X-Content-Type-Options: nosniff`, `Referrer-Policy`, and `frame-ancestors 'none'`.

> [!NOTE]
> Verify the CSP against a production Vite build before shipping it. Module preloading and any inline bootstrap Vite emits will fail under a naive `script-src 'self'`. Resolve that with hashes or a nonce, not by widening the policy to `unsafe-inline`.

## Consequences

### What you gain

- **One artifact, one version.** The binary contains the exact frontend built against the exact OpenAPI document it serves. Frontend and backend cannot be out of step in a deployment, and rollback is redeploying one image tag.
- **The cookie model just works.** No CORS configuration, no preflight, no `SameSite=None`, no credentialed-`EventSource` special case. Same-origin removes an entire class of configuration.
- **Nothing to operate.** No static-file container, no volume to mount for assets, no web server config to maintain alongside the API.
- **Assets cannot drift from the binary at runtime.** They are read-only inside the executable, so there is no directory an operator can half-update and no partially deployed frontend state.
- **Deep links work without server config.** The HTML fallback is a route in the router, not an `nginx` `try_files` rule that has to be rediscovered on every deployment.

### What it costs you

- **Frontend changes invalidate the Rust build.** A one-character CSS change means recompiling the crate that embeds the assets. In release builds that is the slowest part of the loop, which is exactly why day-to-day development runs the Vite dev server against a separately built API instead.
- **Node is required to build the binary.** The Rust release build depends on the Node stage having run, so the Dockerfile stages are ordered and cannot be reversed or skipped.
- **The binary grows by roughly the size of `dist`.** For an SPA this is single-digit megabytes, which is acceptable, but `size-limit` on the main chunk stops being only a frontend performance guard and becomes a check on the artifact as well.
- **No CDN, and no independent frontend deploy.** A UI-only fix requires a full release. For a single-operator self-hosted application this is the correct trade; it would not be for a multi-tenant service.
- **The fallback route is a sharp edge.** If it is ever mounted before the API routes, or widened to all methods, it will swallow API 404s and mask client bugs. Cover it with a test asserting that `GET /api/v1/does-not-exist` returns problem+json.

### Follow-up work this decision creates

1. **Add the fallback-precedence test:** an unknown `/api/v1/*` path returns problem+json `404`, and an unknown non-API path returns `index.html` with `200`.
2. **Add a cache-header test** asserting `immutable` on a hashed asset and `no-cache` on `index.html`, since getting this backwards produces a stale UI that survives a redeploy.
3. **Build and verify the CSP** against a production Vite bundle, and add a smoke check that the app boots with the policy applied.
4. **Confirm the debug-build workflow** reads `web/dist` from disk, so contributors are not forced through a release build.
5. **Keep the `openapi.json` staleness check** in CI as the mechanism that makes the single-artifact guarantee meaningful.

## Alternatives considered

| Alternative | Origin | Artifacts | Outcome |
|---|---|---|---|
| Embed with `rust-embed` | Same | One | **Chosen** |
| `ServeDir` from a mounted volume | Same | Two (binary + assets) | Rejected. Reintroduces version skew and a volume to manage. |
| Separate static container behind a proxy | Same, via proxy | Two containers + proxy config | Rejected. Operational cost without a matching benefit. |
| Separate host or CDN | Cross | Two deployments | Rejected. Breaks the cookie model from ADR-0005. |
| `include_dir` instead of `rust-embed` | Same | One | Rejected. Fewer integrations, last released 2024. |

### `ServeDir` from a mounted volume

`tower-http`'s `ServeDir` serving `/app/web` from a bind mount keeps the same origin and removes the rebuild-on-asset-change cost, which is the one real irritation of embedding.

Rejected because it splits the artifact. The binary and the assets become two things that can be updated independently, which is precisely the skew the OpenAPI codegen check exists to catch — except now it can happen after the build, where CI cannot see it. It also adds a volume to the Compose file and a path that must exist and be readable, turning a class of deployment mistake into a blank page.

### A separate static container behind a reverse proxy

Running `caddy` or `nginx` to serve `dist` and reverse-proxy `/api` to the binary preserves the same origin from the browser's point of view, so the cookie model survives. It is the conventional shape for a larger deployment and enables independent frontend deploys.

Rejected on operational cost. It adds a container and a routing configuration to a single-VM deployment, makes the proxy a required component rather than an optional TLS terminator, and puts the SPA fallback rule in proxy config where it is invisible to the test suite. The independent-deploy benefit is not wanted, for the reason given in the context.

### A separate origin with CORS

Rejected outright rather than on balance. It would require `SameSite=None` cookies, a credentialed CORS configuration with an origin allow-list, preflight handling on every mutation, and `withCredentials` on the SSE connection — weakening the authentication posture established in ADR-0005 in exchange for a deployment flexibility this product does not need.

## Revisit this decision when

- **A second client needs the API on another origin**, such as a native mobile app. That does not by itself change how the web UI is delivered, but it does reopen the authentication discussion in ADR-0005 first.
- **Release-build times become a real constraint** because `dist` has grown substantially. The mitigation is build caching and asset splitting before it is a change of delivery model.
- **The deployment grows a CDN or an edge tier**, at which point serving static assets from the origin binary stops being the simplest option.
- **Multiple frontends appear** — for example a separate admin UI — since embedding several bundles in one binary is where this approach starts to strain.

## References

- [`rust-embed`](https://crates.io/crates/rust-embed) — 8.12.0, with an optional `axum` 0.8 integration feature
- [`mime_guess`](https://crates.io/crates/mime_guess)
- [`tower-http`](https://crates.io/crates/tower-http) — `ServeDir`, considered and rejected
- [Vite: static asset handling and content hashing](https://vite.dev/guide/assets)
- [MDN: Content-Security-Policy](https://developer.mozilla.org/docs/Web/HTTP/Headers/Content-Security-Policy)
