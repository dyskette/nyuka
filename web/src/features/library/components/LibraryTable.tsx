import { Trans, useLingui } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useEffect, useRef, useState } from 'react'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'

type MangaSummary = components['schemas']['MangaSummaryDto']

/** The orderings this table offers. A subset of the route's `sort` values. */
export type SortKey = 'title' | 'chapters' | 'updated'

export interface LibraryTableProps {
  /** Every row loaded so far, across pages. */
  items: MangaSummary[]
  sort: SortKey | 'added'
  dir: 'asc' | 'desc'
  selected: ReadonlySet<string>
  onToggle: (id: string) => void
  onToggleAll: () => void
  /** Asks for the next page. Called once as the end comes into view. */
  onReachEnd?: (() => void) | undefined
  /** True while that request is in flight, so the footer can say so. */
  loadingMore?: boolean
  /** Whether the server has more under the current filters. */
  hasMore?: boolean
}

/**
 * Row height in pixels, and the number kept rendered outside the viewport.
 *
 * The height has to match what `h-row` resolves to, or the virtualizer's
 * arithmetic disagrees with the layout and rows drift as you scroll. It is
 * measured rather than assumed — see `useRowHeight`, which reads the token
 * instead of hard-coding 40, because the density control changes it.
 */
const OVERSCAN = 8

/**
 * The library, as a dense table.
 *
 * A table rather than a cover grid because of what a reader is here to do:
 * see how far behind they are and what is downloaded. Both are numbers, and a
 * grid of covers shows neither without a hover. Browse uses the grid, where
 * the cover *is* the information.
 *
 * A real `<table>` rather than a grid of divs: a screen reader announces
 * column headers with each cell, and sortable columns are `<th>` carrying
 * `aria-sort`. Rebuilding that on divs means rebuilding it wrong.
 */
export function LibraryTable({
  items,
  sort,
  dir,
  selected,
  onToggle,
  onToggleAll,
  onReachEnd,
  loadingMore = false,
  hasMore = false,
}: LibraryTableProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const rowHeight = useRowHeight(scrollRef)

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: OVERSCAN,
  })

  const virtualRows = virtualizer.getVirtualItems()
  const total = virtualizer.getTotalSize()
  const first = virtualRows[0]
  const last = virtualRows[virtualRows.length - 1]

  // Padding rows rather than absolute positioning: a `<tr>` taken out of flow
  // stops being a row of its table, and the column headers no longer apply to
  // its cells. Two spacers keep every rendered row a real one.
  const padTop = first ? first.start : 0
  const padBottom = last ? total - last.end : 0

  // Asked for while the last loaded row is still some way off, so the next
  // page usually arrives before the reader reaches it. Guarded on
  // `loadingMore`, or every scroll event during the request asks again.
  useEffect(() => {
    if (!hasMore || loadingMore || onReachEnd === undefined) return
    if (last !== undefined && last.index >= items.length - OVERSCAN) onReachEnd()
  }, [hasMore, loadingMore, onReachEnd, last, items.length])

  if (items.length === 0) return <EmptyLibrary />

  return (
    <div ref={scrollRef} className="h-full overflow-y-auto">
      <table
        className="w-full border-collapse text-sm"
        // The real size, not the rendered one. Without these a screen reader
        // announces "row 3 of 12" in a library of five hundred, because twelve
        // is all that exists in the DOM.
        aria-rowcount={items.length}
      >
        <caption className="sr-only">
          <Trans>Series in your library</Trans>
        </caption>
        <thead>
          <tr className="border-border text-muted-foreground border-b text-left">
            <th scope="col" className="px-cell h-row w-0">
              <SelectAll items={items} selected={selected} onToggleAll={onToggleAll} />
            </th>
            <Th sortKey="title" sort={sort} dir={dir}>
              <Trans>Title</Trans>
            </Th>
            <Th>
              <Trans>Source</Trans>
            </Th>
            <Th sortKey="chapters" sort={sort} dir={dir} align="right">
              <Trans>Chapters</Trans>
            </Th>
            <Th>
              <Trans>Downloaded</Trans>
            </Th>
            <Th sortKey="updated" sort={sort} dir={dir}>
              <Trans>Updated</Trans>
            </Th>
          </tr>
        </thead>
        <tbody>
          {/* The spacers hold the scrollbar at the height of the whole list
              and carry nothing. Biome reads a `tr` as interactive; `row` is an
              ARIA structure role, focusable only inside a `grid`. */}
          {padTop > 0 && (
            // biome-ignore lint/a11y/noAriaHiddenOnFocusable: a `tr` in a plain `table` is not focusable; see above.
            <tr aria-hidden="true">
              <td colSpan={7} style={{ height: padTop }} />
            </tr>
          )}
          {virtualRows.map((virtual) => {
            const manga = items[virtual.index]
            if (manga === undefined) return null
            return (
              <Row
                key={manga.id}
                manga={manga}
                // One-based, and counted from the whole list rather than the
                // rendered window — the two differ by everything scrolled past.
                rowIndex={virtual.index + 1}
                height={rowHeight}
                selected={selected.has(manga.id)}
                onToggle={onToggle}
              />
            )
          })}
          {padBottom > 0 && (
            // biome-ignore lint/a11y/noAriaHiddenOnFocusable: a `tr` in a plain `table` is not focusable; see above.
            <tr aria-hidden="true">
              <td colSpan={7} style={{ height: padBottom }} />
            </tr>
          )}
        </tbody>
      </table>
      {loadingMore && (
        <p className="text-muted-foreground p-4 text-center text-xs" role="status">
          <Trans>Loading more…</Trans>
        </p>
      )}
    </div>
  )
}

/**
 * The row height the stylesheet is actually using.
 *
 * `--row-h` is remapped by the density control (spec-design.md), so a
 * hard-coded 40 would put the virtualizer's arithmetic out of step with the
 * layout in compact mode — rows would drift further out of place the further
 * you scrolled. Read from the element so one source stays authoritative.
 */
function useRowHeight(ref: React.RefObject<HTMLElement | null>): number {
  const [height, setHeight] = useState(FALLBACK_ROW_HEIGHT)

  useEffect(() => {
    const element = ref.current
    if (element === null) return
    const value = getComputedStyle(element).getPropertyValue('--row-h').trim()
    const parsed = Number.parseFloat(value)
    // A stylesheet that has not loaded gives an empty string, and `NaN` as a
    // row height silently collapses the whole list to zero.
    if (Number.isFinite(parsed) && parsed > 0) setHeight(parsed)
  }, [ref])

  return height
}

/** Matches `--row-h` in `theme.css` at the default density. */
const FALLBACK_ROW_HEIGHT = 40

/**
 * The header checkbox.
 *
 * `indeterminate` is a DOM property with no HTML attribute, so React cannot
 * set it from JSX and it has to go through a ref. Without it a partial
 * selection renders as fully unchecked, and clicking the box appears to do
 * nothing on the first press.
 */
function SelectAll({
  items,
  selected,
  onToggleAll,
}: {
  items: MangaSummary[]
  selected: ReadonlySet<string>
  onToggleAll: () => void
}) {
  const ref = useRef<HTMLInputElement>(null)
  const { t } = useLingui()
  const all = items.length > 0 && items.every((manga) => selected.has(manga.id))
  const some = !all && items.some((manga) => selected.has(manga.id))

  useEffect(() => {
    if (ref.current) ref.current.indeterminate = some
  }, [some])

  return (
    <input
      ref={ref}
      type="checkbox"
      className="accent-accent size-4 align-middle"
      checked={all}
      onChange={onToggleAll}
      aria-label={t`Select every series on this page`}
    />
  )
}

/**
 * A column header, sortable when it is given a `sortKey`.
 *
 * The control is a `Link` rather than a button because the ordering lives in
 * the URL: that keeps the back button working, makes a sorted view
 * shareable, and means the header has a real `href` to middle-click.
 */
function Th({
  children,
  sortKey,
  sort,
  dir,
  align = 'left',
}: {
  children: React.ReactNode
  sortKey?: SortKey
  sort?: SortKey | 'added'
  dir?: 'asc' | 'desc'
  align?: 'left' | 'right'
}) {
  const active = sortKey !== undefined && sortKey === sort
  const className = `px-cell h-row text-xs font-medium ${align === 'right' ? 'text-right' : ''}`

  if (sortKey === undefined) {
    return (
      <th scope="col" className={className}>
        {children}
      </th>
    )
  }

  // WAI-ARIA: `aria-sort` belongs on the header cell, and only the one column
  // actually sorted may carry it — putting `none` on every other column is
  // permitted but announces noise on each cell, so the attribute is simply
  // absent there.
  return (
    <th
      scope="col"
      className={className}
      {...(active ? { 'aria-sort': dir === 'asc' ? 'ascending' : 'descending' } : {})}
    >
      <Link
        to="/library"
        search={(previous) => ({
          ...previous,
          sort: sortKey,
          // A second click on the active column reverses it; a different
          // column starts from its own natural direction rather than
          // inheriting the last one — titles read A–Z, recency reads newest
          // first, and carrying the previous direction over gets one of them
          // backwards.
          dir: active ? (dir === 'asc' ? 'desc' : 'asc') : naturalDirection(sortKey),
          // A new ordering invalidates a cursor taken under the old one.
          cursor: undefined,
        })}
        className="hover:text-foreground inline-flex items-center gap-1"
      >
        {children}
        <span aria-hidden="true" className="text-[0.625rem]">
          {active ? (dir === 'asc' ? '▲' : '▼') : ''}
        </span>
      </Link>
    </th>
  )
}

function naturalDirection(sortKey: SortKey): 'asc' | 'desc' {
  return sortKey === 'title' ? 'asc' : 'desc'
}

function Row({
  manga,
  rowIndex,
  height,
  selected,
  onToggle,
}: {
  manga: MangaSummary
  rowIndex: number
  height: number
  selected: boolean
  onToggle: (id: string) => void
}) {
  const { t } = useLingui()

  return (
    <tr
      aria-rowindex={rowIndex}
      // Fixed, because the virtualizer's arithmetic assumes it. A row that
      // grew to fit its content would push every later row out of position.
      style={{ height }}
      className={`border-border hover:bg-surface-raised border-b ${
        selected ? 'bg-accent-soft' : ''
      }`}
    >
      <td className="px-cell h-row">
        <input
          type="checkbox"
          className="accent-accent size-4 align-middle"
          checked={selected}
          onChange={() => onToggle(manga.id)}
          // The title, not "select row": a list of identical labels is
          // unusable in a screen reader's forms list.
          aria-label={t`Select ${manga.title}`}
        />
      </td>
      <td className="px-cell h-row">
        <Link
          to="/library/$mangaId"
          params={{ mangaId: manga.id }}
          search={(previous) => previous}
          className="flex items-center gap-2"
        >
          {/* A colour block rather than the cover: at this row height a
              thumbnail is unreadable, and the mockup uses it as a recognition
              aid rather than as an image. */}
          <span className="bg-surface-raised size-5 shrink-0 rounded-sm" aria-hidden="true" />
          <span className="truncate font-medium">{manga.title}</span>
        </Link>
      </td>
      <td className="px-cell text-muted-foreground h-row truncate">{manga.source_name}</td>
      <td className="px-cell tabular h-row text-right">{manga.chapter_count}</td>
      <td className="px-cell h-row">
        <DownloadProgress downloaded={manga.downloaded_count} total={manga.chapter_count} />
      </td>
      <td className="px-cell text-muted-foreground h-row">
        <time dateTime={manga.updated_at}>{relativeTime(manga.updated_at)}</time>
      </td>
    </tr>
  )
}

/**
 * How much of a series is on disk.
 *
 * The numbers are the accessible content and the bar is decoration, marked
 * `aria-hidden`. A `role="progressbar"` here would announce the same fraction
 * twice, and colour alone never carries the value — which is what ADR-0016
 * requires of every semantic colour.
 */
function DownloadProgress({ downloaded, total }: { downloaded: number; total: number }) {
  // A series whose chapter list has not been fetched yet has no denominator.
  // Rendering 0/0 as a full bar would say "fully downloaded".
  const fraction = total > 0 ? downloaded / total : 0
  const complete = total > 0 && downloaded === total

  return (
    <span className="flex items-center gap-2">
      <span
        className="bg-surface-raised h-1 w-16 shrink-0 overflow-hidden rounded-sm"
        aria-hidden="true"
      >
        <span
          className={`block h-full rounded-sm ${complete ? 'bg-success' : 'bg-accent'}`}
          style={{
            width: `${fraction * 100}%`,
            transition: `width var(--dur-slow) var(--ease-out)`,
          }}
        />
      </span>
      <span className="tabular text-muted-foreground text-xs">
        {downloaded}/{total}
      </span>
    </span>
  )
}

function EmptyLibrary() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>Your library is empty</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>
          Add a source, then browse its catalog to add a series. Everything you add appears here.
        </Trans>
      </p>
    </div>
  )
}
