import type { QueryClient } from '@tanstack/react-query'
import { createRootRouteWithContext, Outlet } from '@tanstack/react-router'
import { AppShell } from '@/shared/components/AppShell'

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
  return (
    <AppShell>
      <Outlet />
    </AppShell>
  )
}
