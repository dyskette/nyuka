import type { QueryClient } from '@tanstack/react-query'
import { createRootRouteWithContext, Outlet } from '@tanstack/react-router'
import { AppShell } from '@/shared/components/AppShell'
import { LiveProvider } from '@/shared/sse/LiveProvider'

/**
 * The root route.
 *
 * The QueryClient travels in router context so route loaders can call
 * `ensureQueryData` without importing a module-level singleton (ADR-0008).
 */
export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
})

function RootLayout() {
  // TODO: the command palette (cmdk), which the mockup binds to ⌘K.
  //
  // `LiveProvider` wraps the shell rather than sitting inside it, so the
  // status bar and every screen read one stream — mounting it per screen
  // would open a second `EventSource` against a six-connection budget
  // (ADR-0010).
  return (
    <LiveProvider>
      <AppShell>
        <Outlet />
      </AppShell>
    </LiveProvider>
  )
}
