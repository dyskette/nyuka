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
import type { ReactNode } from 'react'

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
    component: () => <>{ui}</>,
  })
  // Declared so `Link to="/library/$mangaId"` resolves. Its component is
  // never rendered; what matters is that the route exists to link to.
  const detailRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/library/$mangaId',
    component: () => null,
  })

  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, detailRoute]),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })

  // Awaited before rendering: the router resolves its route asynchronously,
  // so without this the first paint is empty and every query finds nothing —
  // which reads as a component that renders nothing rather than a test that
  // looked too early.
  await router.load()

  return render(
    <I18nProvider i18n={i18n}>
      <QueryClientProvider client={queryClient}>
        {/* The router's types describe the real route tree; this one is a
            stand-in, so the mismatch is asserted away rather than pretended
            not to exist. */}
        <RouterProvider router={router as never} />
      </QueryClientProvider>
    </I18nProvider>,
  )
}
