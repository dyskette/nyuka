import { Trans, useLingui } from '@lingui/react/macro'
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
export function AppShell({
  children,
  onOpenPalette,
}: {
  children: ReactNode
  onOpenPalette?: (() => void) | undefined
}) {
  return (
    <div className="grid h-dvh grid-cols-[13rem_1fr] grid-rows-[1fr_auto]">
      <Sidebar />
      <div className="min-w-0 overflow-hidden">{children}</div>
      <StatusBar onOpenPalette={onOpenPalette} />
    </div>
  )
}

interface NavItem {
  to: string
  icon: LucideIcon
  label: ReactNode
  /** Shown right-aligned, as the mockup shows for Downloads. */
  badge?: number
  /** The keyboard shortcut, in the muted colour — see the note at its `kbd`. */
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
      {/*
        No `opacity` on top of the colour. `--muted-foreground` measures
        5.83:1 on the page, and dimming it to 60% took it to 2.54:1 — under
        the 4.5:1 WCAG 1.4.3 requires of 10px text. That is the same mistake
        ADR-0016 already corrected once, where the focus ring at 60% alpha
        measured 1.92:1. A token that passes on its own does not pass through
        an opacity.
      */}
      <kbd className="text-muted-foreground font-mono text-[10px]">{item.shortcut}</kbd>
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
function StatusBar({ onOpenPalette }: { onOpenPalette?: (() => void) | undefined }) {
  const { t } = useLingui()

  return (
    <footer className="border-border text-muted-foreground col-span-1 flex items-center gap-4 border-t px-3 py-1.5 text-xs">
      <LiveIndicator />
      {/*
        A button, not a label. The shortcut is the fast path, but advertising
        one without offering a way to press it leaves the palette unreachable
        to anyone who cannot make that chord — and to a touch device, which
        has no keyboard at all (WCAG 2.5.1).
      */}
      <button
        type="button"
        onClick={onOpenPalette}
        aria-label={t`Open the command palette`}
        aria-keyshortcuts="Meta+K Control+K"
        className="hover:text-foreground tabular ml-auto rounded-sm"
      >
        ⌘K
      </button>
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
