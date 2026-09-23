import { Trans } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import { BookMarked, Compass, Download, Library, type LucideIcon, Settings } from 'lucide-react'
import type { ReactNode } from 'react'
import { useLiveStatus } from '@/shared/sse/LiveProvider'

/**
 * The frame every screen sits in: a fixed sidebar, the screen, and a status
 * bar.
 *
 * The status bar is not decoration. It carries the two things that are true
 * regardless of which screen is open — whether the event stream is connected,
 * and what the download queue is doing — so a user never has to navigate to
 * Downloads to find out that nothing is progressing.
 */
export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="grid h-dvh grid-cols-[13rem_1fr] grid-rows-[1fr_auto]">
      <Sidebar />
      <div className="min-w-0 overflow-hidden">{children}</div>
      <StatusBar />
    </div>
  )
}

interface NavItem {
  to: string
  icon: LucideIcon
  label: ReactNode
  /** Shown right-aligned, as the mockup shows for Downloads. */
  badge?: number
  /** The keyboard shortcut, rendered dimmed. */
  shortcut: string
}

function Sidebar() {
  const items: NavItem[] = [
    { to: '/library', icon: Library, label: <Trans>Library</Trans>, shortcut: 'g l' },
    { to: '/browse', icon: Compass, label: <Trans>Browse</Trans>, shortcut: 'g b' },
    { to: '/downloads', icon: Download, label: <Trans>Downloads</Trans>, shortcut: 'g d' },
    { to: '/follows', icon: BookMarked, label: <Trans>Follows</Trans>, shortcut: 'g f' },
  ]

  return (
    <nav className="border-border row-span-2 flex flex-col border-r" aria-label="Main">
      <div className="flex items-center gap-2 px-3 py-3">
        <div className="bg-accent size-4 rounded-sm" aria-hidden="true" />
        <span className="text-sm font-medium">nyuka</span>
      </div>

      <ul className="flex flex-col gap-0.5 px-2">
        {items.map((item) => (
          <li key={item.to}>
            <SidebarLink item={item} />
          </li>
        ))}
      </ul>

      <div className="mt-auto px-2 pb-2">
        <SidebarLink
          item={{
            to: '/settings',
            icon: Settings,
            label: <Trans>Settings</Trans>,
            shortcut: 'g ,',
          }}
        />
      </div>
    </nav>
  )
}

function SidebarLink({ item }: { item: NavItem }) {
  const Icon = item.icon
  return (
    <Link
      to={item.to}
      className="text-muted-foreground hover:bg-surface-raised flex items-center gap-2 rounded px-2 py-1.5 text-sm"
      // `activeProps` rather than a className callback: the router owns which
      // link is current, and `aria-current` is what a screen reader reads —
      // a colour change alone would leave that user with no indication at all.
      activeProps={{
        className:
          'bg-accent-soft text-foreground flex items-center gap-2 rounded px-2 py-1.5 text-sm',
        'aria-current': 'page',
      }}
    >
      <Icon className="size-4 shrink-0" aria-hidden="true" />
      <span className="flex-1 truncate">{item.label}</span>
      {item.badge !== undefined && item.badge > 0 && (
        <span className="bg-accent text-accent-foreground tabular rounded px-1.5 text-xs">
          {item.badge}
        </span>
      )}
      <kbd className="text-muted-foreground font-mono text-[10px] opacity-60">{item.shortcut}</kbd>
    </Link>
  )
}

/**
 * The status bar.
 *
 * Counters use `tabular` so the row does not reflow as digits change — a
 * number that jitters while it counts is harder to read than one that does
 * not, and this one updates continuously during a download.
 */
function StatusBar() {
  return (
    <footer className="border-border text-muted-foreground col-span-1 flex items-center gap-4 border-t px-3 py-1.5 text-xs">
      <LiveIndicator />
      <span className="ml-auto tabular">
        <Trans>⌘K</Trans>
      </span>
    </footer>
  )
}

/**
 * Whether the event stream is connected.
 *
 * Read from the provider rather than hard-coded: a status bar that always says
 * "Live" is worse than none, because the one moment it matters is the one
 * where it is wrong.
 */
function LiveIndicator() {
  const status = useLiveStatus()

  // The indicator is a dot *and* a word: colour alone would leave the state
  // unreadable to anyone who cannot distinguish it (WCAG 1.4.1).
  const dot = {
    live: 'bg-success',
    connecting: 'bg-warning',
    offline: 'bg-danger',
  }[status]

  return (
    <span className="flex items-center gap-1.5">
      <span className={`${dot} size-1.5 rounded-full`} aria-hidden="true" />
      {/* `aria-live` so the change is announced: losing the connection is the
          kind of thing a reader needs to hear rather than notice. */}
      <span aria-live="polite">
        {status === 'live' ? (
          <Trans>Live</Trans>
        ) : status === 'connecting' ? (
          <Trans>Connecting</Trans>
        ) : (
          <Trans>Offline</Trans>
        )}
      </span>
    </span>
  )
}
