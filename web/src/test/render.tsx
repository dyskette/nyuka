import { i18n } from '@lingui/core'
import { I18nProvider } from '@lingui/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { render } from '@testing-library/react'
import { createContext, type ReactNode, use } from 'react'

/**
 * Carries the component under test down to the index route.
 *
 * Not captured in the route's own component closure: the router is built once
 * per call, so a closed-over node is frozen at the first render and a
 * `rerender` with new props would change nothing — the test would pass or
 * fail against the original props while appearing to test the new ones.
 */
const UnderTest = createContext<ReactNode>(null)

function RenderUnderTest() {
  return <>{use(UnderTest)}</>
}

/**
 * Renders a component inside the providers it expects.
 *
 * A router is included because components use `Link`, which throws outside
 * one. Using a real router with a memory history rather than mocking `Link`
 * means a wrong `to` or a missing param is a test failure here rather than a
 * broken href in production.
 */
export async function renderWithProviders(ui: ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: {
      // Tests assert on what a failure renders, so a retry would only make
      // them slower before showing the same thing.
      queries: { retry: false },
    },
  })

  const rootRoute = createRootRoute()
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/',
    component: RenderUnderTest,
  })
  // Declared so `Link to="/library/$mangaId"` resolves. Its component is
  // never rendered; what matters is that the route exists to link to.
  const detailRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/library/$mangaId',
    component: () => null,
  })

  // Same, for `Link to="/library"` — the sortable column headers. A route
  // that does not exist makes `Link` throw, so this is not decoration.
  const listRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/library',
    component: () => null,
  })

  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, listRoute, detailRoute]),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })

  // Awaited before rendering: the router resolves its route asynchronously,
  // so without this the first paint is empty and every query finds nothing —
  // which reads as a component that renders nothing rather than a test that
  // looked too early.
  await router.load()

  const wrap = (node: ReactNode) => (
    <I18nProvider i18n={i18n}>
      <QueryClientProvider client={queryClient}>
        <UnderTest value={node}>
          {/* The router's types describe the real route tree; this one is a
              stand-in, so the mismatch is asserted away rather than pretended
              not to exist. */}
          <RouterProvider router={router as never} />
        </UnderTest>
      </QueryClientProvider>
    </I18nProvider>
  )

  const result = render(wrap(ui))

  return {
    ...result,
    // Overridden, because React Testing Library's own `rerender` replaces the
    // whole tree with exactly what it is handed — so a component rerendered
    // through it loses every provider and fails with "useLingui was used
    // without I18nProvider". Re-wrapping here keeps a rerender a prop change
    // rather than a different test.
    rerender: (next: ReactNode) => result.rerender(wrap(next)),
  }
}
