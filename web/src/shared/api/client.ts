import createClient, { type Middleware } from 'openapi-fetch'
import type { paths } from './schema'

/**
 * The typed API client.
 *
 * `paths` is generated from the server's own OpenAPI document by
 * `cargo xtask openapi`, and CI fails when either that file or this schema is
 * stale. So a route that no longer exists is a type error here rather than a
 * 404 at runtime — which is the whole reason the schema is generated rather
 * than hand-written (ADR-0009).
 */

/** The header the server requires on every state-changing request. */
export const CSRF_HEADER = 'x-requested-with'

/**
 * Sent on every mutation.
 *
 * The value is never inspected by the server: an attacker who could set the
 * header could set any value, so presence is the whole signal. What matters is
 * that a cross-site form cannot set a custom header at all, and a cross-origin
 * `fetch` that tries triggers a preflight this server does not answer
 * (ADR-0005).
 */
const CSRF_VALUE = 'XMLHttpRequest'

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS', 'TRACE'])

/**
 * Adds the CSRF header to every unsafe request.
 *
 * In middleware rather than at each call site, for the same reason the server
 * enforces it in middleware: a single forgotten header would be a mutation
 * that fails with a 403 nobody expects, and a per-call convention is one
 * someone eventually forgets.
 */
const csrf: Middleware = {
  async onRequest({ request }) {
    if (!SAFE_METHODS.has(request.method.toUpperCase())) {
      request.headers.set(CSRF_HEADER, CSRF_VALUE)
    }
    return request
  },
}

export const api = createClient<paths>({
  baseUrl: '/api/v1',

  // The session is an HttpOnly cookie, so there is no token to attach and
  // nothing to read from JavaScript. `same-origin` is the default for
  // same-origin requests, but stating it makes the dependency on the cookie
  // explicit — and the Vite dev proxy keeps `changeOrigin` false precisely so
  // this stays same-origin in development too (ADR-0005, ADR-0006).
  credentials: 'same-origin',
})

api.use(csrf)
