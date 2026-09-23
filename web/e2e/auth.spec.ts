import { expect, test } from '@playwright/test'
import { REFUSING_ORIGIN, SESSION_ORIGIN } from '../playwright.config'

/**
 * The OIDC paths that need a provider.
 *
 * ADR-0005 lists these explicitly because none is exercised by ordinary
 * feature work and each is a real vulnerability when it is wrong.
 * `crates/api/tests/auth.rs` covers the ones reachable without a token
 * exchange and names the rest as belonging here.
 */

const LOGIN = '/api/v1/auth/login'
const CALLBACK = '/api/v1/auth/callback'

test.describe('signing in', () => {
  /**
   * One test, not two, because each sign-in spends from the rate limiter's
   * budget and the suite has to fit inside it.
   */
  test('a permitted subject gets a session that works', async ({ page }) => {
    await page.goto(LOGIN)

    // The whole redirect chain: this server, the provider, back again. Landing
    // anywhere on the app means the callback set a session.
    await expect(page).toHaveURL(/127\.0\.0\.1:18080/)

    const me = await page.request.get('/api/v1/me')
    expect(me.ok()).toBe(true)
    expect(await me.json()).toMatchObject({ subject: 'tester' })

    // And it reaches a protected route, which is the point of having one.
    expect((await page.request.get('/api/v1/manga')).status()).toBe(200)
  })

  /**
   * The allow-list is consulted by the callback, not merely present in the
   * configuration. `AuthConfig::permits` is unit-tested; that the callback
   * asks it is what only an end-to-end run can show.
   */
  test('a subject absent from the allow-list is refused', async ({ page }) => {
    const response = await page.goto(`${REFUSING_ORIGIN}${LOGIN}`)

    expect(response?.status()).toBe(403)

    // And no session was created for it.
    const me = await page.request.get(`${REFUSING_ORIGIN}/api/v1/me`)
    expect(me.status()).toBe(401)
  })
})

test.describe('the callback refuses what it cannot verify', () => {
  /**
   * The `state` binds the callback to the browser that started the flow.
   * Without the check, an attacker's callback completes in a victim's session
   * and logs them into the attacker's account.
   */
  test('a state that does not match the session', async ({ page }) => {
    // Start a real flow, so the session holds a state — then answer with a
    // different one.
    const authorize = await authorizeUrl(page)
    const code = await codeFrom(page, authorize)
    const response = await page.request.get(`${CALLBACK}?code=${code}&state=not-the-one`, {
      maxRedirects: 0,
    })

    expect(response.status()).toBe(400)
    expect(await response.json()).toMatchObject({ type: '/problems/state-mismatch' })
  })

  /**
   * A callback with no flow in progress. The session holds no state, so there
   * is nothing to compare and the request cannot be the end of a sign-in.
   */
  test('a callback with nothing in progress', async ({ page }) => {
    const response = await page.request.get(`${CALLBACK}?code=made-up&state=made-up`, {
      maxRedirects: 0,
    })

    expect(response.status()).toBe(400)
  })

  /**
   * A code is single-use at the provider. Replaying one must fail at the token
   * exchange rather than mint a second session — which is what the `nonce` and
   * the provider's own code expiry are between them for.
   */
  test('an authorization code replayed after it was spent', async ({ page }) => {
    const authorize = await authorizeUrl(page)
    const code = await codeFrom(page, authorize)

    const first = await page.request.get(`${CALLBACK}?code=${code}&state=${stateFrom(authorize)}`, {
      maxRedirects: 0,
    })
    expect(first.status()).toBeLessThan(400)

    // The same code again, in the same browser.
    const second = await page.request.get(
      `${CALLBACK}?code=${code}&state=${stateFrom(authorize)}`,
      { maxRedirects: 0 },
    )
    expect(second.status()).toBeGreaterThanOrEqual(400)
  })
})

/**
 * On their own server, because each of these needs a full sign-in and the
 * limiter's budget on the first one is spent by the tests above.
 */
test.describe('signing out', () => {
  const ME = `${SESSION_ORIGIN}/api/v1/me`
  const LOGOUT = `${SESSION_ORIGIN}/api/v1/auth/logout`

  /**
   * One sign-in covers both halves: a cross-site request must not be able to
   * sign anyone out, and a proper one must.
   */
  test('a session survives a forged sign-out and not a real one', async ({ page }) => {
    await page.goto(`${SESSION_ORIGIN}${LOGIN}`)
    expect((await page.request.get(ME)).ok()).toBe(true)

    // No CSRF header: refused, and the session is untouched.
    expect((await page.request.post(LOGOUT, { maxRedirects: 0 })).status()).toBe(403)
    expect((await page.request.get(ME)).ok()).toBe(true)

    // With it: accepted, and the session is gone.
    const out = await page.request.post(LOGOUT, {
      headers: { 'x-requested-with': 'XMLHttpRequest' },
    })
    expect(out.ok()).toBe(true)
    expect((await page.request.get(ME)).status()).toBe(401)
  })
})

/**
 * Starts a sign-in and returns where the server sent the browser.
 *
 * The session cookie the response sets is what holds the `state`, the PKCE
 * verifier and the nonce, so every test below depends on this having happened
 * in the same browser context.
 */
async function authorizeUrl(page: import('@playwright/test').Page): Promise<string> {
  const start = await page.request.get(LOGIN, { maxRedirects: 0 })
  const location = start.headers().location
  expect(location, 'signing in must redirect to the provider').toBeTruthy()
  expect(location).toContain('/authorize')
  return location as string
}

/** Drives the stub's `/authorize` and returns the code it hands back. */
async function codeFrom(page: import('@playwright/test').Page, authorize: string) {
  const response = await page.request.get(authorize, { maxRedirects: 0 })
  const location = response.headers().location
  expect(location, 'the stub must redirect back to the callback').toBeTruthy()
  const code = new URL(location as string).searchParams.get('code')
  expect(code, 'the stub must return an authorization code').toBeTruthy()
  return code as string
}

function stateFrom(authorize: string): string {
  return new URL(authorize).searchParams.get('state') ?? ''
}

/**
 * The flow's secrets are consumed on first use, not on success.
 *
 * `callback` removes the state, the nonce and the PKCE verifier from the
 * session before it validates anything, so a second callback finds nothing to
 * validate against whichever branch the first one took. The code says so in a
 * comment; this is what holds it.
 *
 * The first callback here carries a **bogus code with the correct state**, so
 * it fails at the token exchange. That matters: the provider never sees a
 * valid code, so it cannot be the provider's single-use enforcement that
 * refuses the second attempt. Only the session consumption can.
 *
 * This is the distinction the "replayed code" test above cannot draw, because
 * there the code really was spent.
 */
test('a flow cannot be replayed even when it never completed', async ({ page }) => {
  const start = await page.request.get(`${SESSION_ORIGIN}${LOGIN}`, { maxRedirects: 0 })
  const authorize = start.headers().location as string
  const state = new URL(authorize).searchParams.get('state') ?? ''
  expect(state).not.toBe('')

  const callback = `${SESSION_ORIGIN}${CALLBACK}?code=never-issued&state=${state}`

  // Fails at the exchange: the state matched, so it got that far.
  const first = await page.request.get(callback, { maxRedirects: 0 })
  expect(first.status()).toBe(502)

  // The same state again. The secrets are gone, so this cannot even reach the
  // exchange — a different failure, which is the point.
  const second = await page.request.get(callback, { maxRedirects: 0 })
  expect(second.status()).toBe(400)
})

/**
 * The sign-in rate limit (ADR-0005 follow-up 6).
 *
 * Deliberately last in the file: it exhausts one server's budget, and
 * anything signing in there afterwards would get a 429 it did not earn.
 * Playwright runs a file's tests in declaration order with one worker, which
 * is what makes "last" mean anything.
 *
 * Asserted rather than worked around. Writing the suite to stay under the
 * limit without saying so leaves the next person to discover it by watching
 * the final test fail for no visible reason — which is how it was discovered
 * here.
 */
test('sign-in is rate limited', async ({ page }) => {
  const statuses: number[] = []
  for (let attempt = 0; attempt < 20; attempt += 1) {
    // On the refusing server, whose budget this suite barely touches. The
    // limiter runs before the allow-list, so a subject that will be refused
    // still exercises it.
    const response = await page.request.get(`${REFUSING_ORIGIN}${LOGIN}`, { maxRedirects: 0 })
    statuses.push(response.status())
    if (response.status() === 429) break
  }

  expect(statuses).toContain(429)
  // And it let a reasonable number through first: a limiter that refused the
  // first attempt would lock everyone out rather than slow an attacker down.
  expect(statuses.filter((status) => status === 303).length).toBeGreaterThan(3)
})
