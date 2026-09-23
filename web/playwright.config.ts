import { defineConfig, devices } from '@playwright/test'

/**
 * End-to-end configuration.
 *
 * These tests exist for one thing Vitest cannot reach: the OIDC negative paths
 * ADR-0005 lists, each of which needs a real token exchange with a real
 * provider. `crates/api/tests/auth.rs` covers the ones that do not.
 *
 * # What has to be running
 *
 * - The stub provider, from `e2e/compose.yaml`:
 *   `podman compose -f e2e/compose.yaml up -d` (or `docker compose`).
 * - A PostgreSQL named by `DATABASE_URL`, which the run creates databases on.
 * - A built frontend and a built API: `npm run build && cargo build -p nyuka-api`.
 *
 * The servers below are started by Playwright; the stub and the database are
 * not, because both are shared with the rest of the test suite and starting
 * them per run would fight whatever else is using them.
 */
/**
 * Three instances, because the sign-in rate limit is per process.
 *
 * `/auth/login` and `/auth/callback` share one limiter with a burst of ten
 * (ADR-0005 follow-up 6), so a full sign-in costs two and a suite of eight
 * tests does not fit in one server's budget. Splitting by what each test needs
 * is what keeps any of them from failing on a 429 it did not earn — and the
 * alternative, arithmetic that keeps the whole suite under ten, breaks the
 * moment anyone adds a test.
 */
const ALLOWED_PORT = 18080
const REFUSING_PORT = 18081
const SESSION_PORT = 18082

export default defineConfig({
  testDir: './e2e',
  // A failing end-to-end test is a real failure. Retrying hides a flake
  // rather than reporting it, and the flakes worth having here are the ones
  // that get fixed.
  retries: 0,
  // Serial. Two instances share one stub provider and one PostgreSQL server,
  // and a sign-in is not something two tests can be doing at once without
  // each seeing the other's session.
  workers: 1,
  fullyParallel: false,
  reporter: process.env.CI ? 'github' : 'list',

  use: {
    baseURL: `http://127.0.0.1:${ALLOWED_PORT}`,
    trace: 'retain-on-failure',
    // The app is same-origin with the API (ADR-0006), so nothing here needs
    // to relax cookie handling.
  },

  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],

  webServer: [
    {
      command: `bash e2e/serve.sh ${ALLOWED_PORT} tester nyuka_e2e_allowed`,
      url: `http://127.0.0.1:${ALLOWED_PORT}/healthz`,
      // Never reused, even locally. `/auth/login` is rate limited (ADR-0005
      // follow-up 6) and the limiter's bucket is per process — a server kept
      // between runs carries the previous run's sign-ins, so the suite starts
      // partway through its own budget and the last tests get a 429 they did
      // not earn. A fresh server costs a couple of seconds.
      reuseExistingServer: false,
      stdout: 'pipe',
      stderr: 'pipe',
    },
    {
      // Identical but for the allow-list. The stub issues one fixed subject,
      // so admitting it and refusing it cannot be the same server.
      //
      // Also where the rate-limit test runs: the suite spends one request
      // here, so there is a budget left to exhaust deliberately. Rate limiting
      // happens before the allow-list is consulted, so a refused subject still
      // exercises it.
      command: `bash e2e/serve.sh ${REFUSING_PORT} somebody-else nyuka_e2e_refusing`,
      url: `http://127.0.0.1:${REFUSING_PORT}/healthz`,
      reuseExistingServer: false,
      stdout: 'pipe',
      stderr: 'pipe',
    },
    {
      // Session lifecycle. Sharing the first server would leave these last in
      // the file and first to hit the limit.
      command: `bash e2e/serve.sh ${SESSION_PORT} tester nyuka_e2e_session`,
      url: `http://127.0.0.1:${SESSION_PORT}/healthz`,
      reuseExistingServer: false,
      stdout: 'pipe',
      stderr: 'pipe',
    },
  ],
})

/** The server that refuses every subject, and where the limiter is tested. */
export const REFUSING_ORIGIN = `http://127.0.0.1:${REFUSING_PORT}`

/** The server the session-lifecycle tests use. */
export const SESSION_ORIGIN = `http://127.0.0.1:${SESSION_PORT}`
