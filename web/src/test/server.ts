import { setupServer } from 'msw/node'
import { createOpenApiHttp } from 'openapi-msw'
import type { paths } from '@/shared/api/schema'

/**
 * The mock API for component tests.
 *
 * MSW rather than a `fetch` stub. `openapi-fetch` captures `globalThis.fetch`
 * when the client is created, which happens the moment a test file imports it
 * — so `vi.stubGlobal('fetch', …)` afterwards replaces a reference nothing
 * reads, and the request goes to the real network instead. MSW intercepts
 * below that, so when the reference was captured does not matter.
 *
 * `createOpenApiHttp` types every handler against the generated `paths`, so a
 * handler for a route that does not exist, or one returning a body the schema
 * forbids, is a type error rather than a test that passes against a fiction.
 */
export const http = createOpenApiHttp<paths>({ baseUrl: '/api/v1' })

/**
 * No default handlers.
 *
 * `onUnhandledRequest: 'error'` in `setup.ts` means a test that forgets to
 * declare what the server returns fails loudly, rather than quietly rendering
 * an empty state that looks like a passing assertion.
 */
export const server = setupServer()
