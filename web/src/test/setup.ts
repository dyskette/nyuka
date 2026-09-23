import { i18n } from '@lingui/core'
// Registers the DOM matchers (`toBeInTheDocument`, `toHaveAttribute`,
// `toHaveStyle`) on Vitest's `expect`. Imported for the side effect: without
// it those matchers do not exist, and using one fails with "Invalid Chai
// property" rather than with the assertion it was meant to make.
import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterAll, afterEach, beforeAll } from 'vitest'
import { server } from './server'

/**
 * Resolve relative request URLs against the document's origin.
 *
 * The API client uses a relative base URL, because the frontend is served from
 * the same origin as the API (ADR-0006). A browser resolves that against the
 * document. Node's `fetch`, which Vitest exposes in jsdom's place, refuses it:
 *
 *     TypeError: Failed to parse URL from /api/v1/manga
 *
 * That rejects inside the client, so it reads as a request that returned
 * nothing rather than one that was never made.
 *
 * At module scope, not in `beforeAll`: `openapi-fetch` captures `globalThis
 * .fetch` when the client is created, which happens when a test file imports
 * it — after this file runs, but before any hook in it.
 */
// The document's own origin, exactly as a browser would resolve against. A
// hard-coded one differs from jsdom's `http://localhost:3000` and MSW then
// fails to match handlers written as relative paths.
const ORIGIN = window.location.origin

function absolute(input: RequestInfo | URL): RequestInfo | URL {
  return typeof input === 'string' && input.startsWith('/') ? new URL(input, ORIGIN) : input
}

// `openapi-fetch` builds a `Request` before calling `fetch`, and hands it to
// middleware — so this is the constructor that actually throws. `fetch` is
// wrapped too, for any caller that skips the client.
const NativeRequest = globalThis.Request
globalThis.Request = class extends NativeRequest {
  constructor(input: RequestInfo | URL, init?: RequestInit) {
    super(absolute(input), init)
  }
} as typeof Request

/**
 * Test environment setup.
 *
 * The catalog is activated with the source messages rather than a compiled
 * one: a component test asserts on the English a developer wrote, so it does
 * not break every time a translation is added — and it does not need a build
 * step to run.
 */
beforeAll(() => {
  i18n.loadAndActivate({ locale: 'en', messages: {} })

  // jsdom does not implement scrolling and logs "Not implemented" for every
  // router navigation. Stubbed rather than filtered from the output, because
  // a real error in that stream should still stand out.
  window.scrollTo = () => {}

  // jsdom performs no layout, so every element measures 0 x 0. A virtualizer
  // asks its scroll container how tall it is, concludes nothing is visible,
  // and renders no rows at all — the table appears empty while every other
  // assertion about it still holds.
  //
  // TanStack Virtual reads `offsetWidth`/`offsetHeight`, not
  // `getBoundingClientRect`, so shimming only the latter leaves it measuring
  // zero. Both are given a fixed viewport size.
  //
  // This means a test here can assert *what* is rendered and never *where*:
  // nothing in this environment measures anything, and a test that appeared
  // to check a position would be checking these constants.
  for (const [property, value] of [
    ['offsetWidth', 1024],
    ['offsetHeight', 768],
    ['clientWidth', 1024],
    ['clientHeight', 768],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, property, {
      configurable: true,
      get: () => value,
    })
  }

  Element.prototype.getBoundingClientRect = () =>
    ({
      width: 1024,
      height: 768,
      top: 0,
      left: 0,
      bottom: 768,
      right: 1024,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }) as DOMRect

  // Also absent for want of layout: `cmdk` scrolls the highlighted item into
  // view every time the selection moves, so without this every keyboard
  // interaction in the palette throws.
  Element.prototype.scrollIntoView = () => {}

  // jsdom implements no layout, so it has no `ResizeObserver` — `cmdk` uses
  // one to size its list and throws without it. A no-op is correct here rather
  // than a lie: there is nothing to observe, because nothing has a size.
  //
  // This shim means these tests cannot assert anything about measurement. They
  // assert what is rendered and what is selected, which is what they are for.
  if (!('ResizeObserver' in globalThis)) {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
  }
})

/**
 * The mock API.
 *
 * `onUnhandledRequest: 'error'` so a test that does not say what the server
 * returns fails rather than rendering an empty state that reads as a passing
 * assertion.
 */
beforeAll(() => server.listen({ onUnhandledRequest: 'error' }))
afterEach(() => server.resetHandlers())
afterAll(() => server.close())

// Without this, a component from one test is still in the document during the
// next, and a query for "the heading" matches two — which reads as a
// component bug rather than a leaked render.
afterEach(cleanup)
