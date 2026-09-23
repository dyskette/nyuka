import type { QueryClient } from '@tanstack/react-query'
import { createRootRouteWithContext, Outlet } from '@tanstack/react-router'
import { useCallback, useState } from 'react'
import { CommandPalette } from '@/features/palette/components/CommandPalette'
import { usePaletteHotkey } from '@/features/palette/usePaletteHotkey'
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
  // The palette lives at the root because its hotkey is global and it acts on
  // every screen. Its own data is fetched only while it is open.
  const [paletteOpen, setPaletteOpen] = useState(false)
  const togglePalette = useCallback(() => setPaletteOpen((open) => !open), [])
  usePaletteHotkey(togglePalette)

  // `LiveProvider` wraps the shell rather than sitting inside it, so the
  // status bar and every screen read one stream — mounting it per screen
  // would open a second `EventSource` against a six-connection budget
  // (ADR-0010).
  return (
    <LiveProvider>
      <AppShell onOpenPalette={togglePalette}>
        <Outlet />
      </AppShell>
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
    </LiveProvider>
  )
}
