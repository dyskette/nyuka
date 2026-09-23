import { I18nProvider } from '@lingui/react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createRouter, RouterProvider } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './app/theme.css'
import { routeTree } from './routeTree.gen'
import { isProblem } from './shared/api/problem'
import { activate, i18n, negotiate } from './shared/i18n'

// Per-resource staleTime lives with each feature's hooks (ADR-0009); these are
// only the defaults.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // One retry, and never for a failure the server has already explained.
      // Retrying a 401 or a 404 cannot succeed, and doing so turns one clear
      // failure into a slower identical one.
      retry: (failureCount, error) => {
        if (isProblem(error) && error.status >= 400 && error.status < 500) return false
        return failureCount < 1
      },
      // SSE is the live channel (ADR-0010). Refetching on focus on top of it
      // is the polling that decision exists to avoid.
      refetchOnWindowFocus: false,
    },
  },
})

const router = createRouter({ routeTree, context: { queryClient } })

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

// TODO: the SSE provider inside the auth gate — the EventSource opens only
// after authentication succeeds (ADR-0010).
async function start() {
  // Activated before the first render, so no component ever renders an
  // untranslated string and then swaps it. `failOnMissing` makes a missing
  // message a build failure, so the only runtime question is which catalog to
  // load, not whether it is complete (ADR-0011).
  await activate(negotiate(navigator.languages))

  const root = document.getElementById('root')
  if (!root) throw new Error('no #root element to mount into')

  createRoot(root).render(
    <StrictMode>
      <I18nProvider i18n={i18n}>
        <QueryClientProvider client={queryClient}>
          <RouterProvider router={router} />
        </QueryClientProvider>
      </I18nProvider>
    </StrictMode>,
  )
}

void start()
