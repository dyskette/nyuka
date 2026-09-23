import { Trans } from '@lingui/react/macro'
import { useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Outlet, retainSearchParams } from '@tanstack/react-router'
import { useCallback, useMemo, useState } from 'react'
import { z } from 'zod'
import { libraryListQuery } from '@/features/library/api/queries'
import { LibraryTable } from '@/features/library/components/LibraryTable'
import { type LibraryFilters, LibraryToolbar } from '@/features/library/components/LibraryToolbar'
import type { components } from '@/shared/api/schema'

type MangaSummary = components['schemas']['MangaSummaryDto']

/**
 * The library master list, and the parent of the detail panel (ADR-0017).
 *
 * Search parameters are validated here, on the parent, so the detail route
 * inherits them and `retainSearchParams` can preserve them across opening and
 * closing the panel.
 */
const searchSchema = z.object({
  q: z.string().optional(),
  // Empty string is not a value these can take — an absent filter is an
  // absent key, so a default-valued library has a clean URL.
  status: z.string().optional(),
  source: z.string().optional(),
  sort: z.enum(['title', 'updated', 'added', 'chapters']).default('updated'),
  dir: z.enum(['asc', 'desc']).default('desc'),
  cursor: z.string().optional(),
})

export const Route = createFileRoute('/library')({
  // Zod 4 implements Standard Schema, so the schema is passed directly.
  // Do NOT add @tanstack/zod-adapter — it does not work with Zod 4 (ADR-0008).
  validateSearch: searchSchema,

  // Without this, navigating to /library/$mangaId drops these from the URL,
  // the parent re-defaults them, and the user's filtered view silently resets
  // — while the list component stays mounted, so it reads as a data bug rather
  // than a routing one (ADR-0017).
  search: {
    middlewares: [retainSearchParams(['q', 'status', 'source', 'sort', 'dir', 'cursor'])],
  },

  // Declared before `loader`, because that is what `deps` is inferred from.
  //
  // Only the cursor affects what is fetched. Sort and direction are applied to
  // what is already loaded, so listing them here would refetch the same page
  // every time the user changed the ordering.
  loaderDeps: ({ search }) => ({ cursor: search.cursor }),

  // Primes the cache so the table has data on first paint. The loader returns
  // nothing the component reads: components always read through hooks, so
  // there is one source of truth for the data and one for its loading state
  // (ADR-0008).
  loader: ({ context, deps }) => context.queryClient.ensureQueryData(libraryListQuery(deps.cursor)),

  component: LibraryLayout,
  pendingComponent: LibrarySkeleton,
  errorComponent: LibraryError,
})

function LibraryLayout() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const { data } = useSuspenseQuery(libraryListQuery(search.cursor))

  // The three filter fields are destructured rather than passing `search`
  // whole: a memo depending on the whole object also recomputes when the
  // cursor changes, and the dependency list would no longer describe what the
  // computation actually reads.
  const { q, status, source } = search
  const items = useMemo(
    () => filterItems(sortItems(data.items, search.sort, search.dir), { q, status, source }),
    [data.items, search.sort, search.dir, q, status, source],
  )

  // Offered from what is on the page rather than from a fixed list: a source
  // the reader has not installed is not a filter worth showing, and an
  // enumeration of every possible status would offer several that match
  // nothing.
  const sources = useMemo(() => distinct(data.items.map((m) => m.source_name)), [data.items])
  const statuses = useMemo(() => distinct(data.items.map((m) => m.status)), [data.items])

  const onFilterChange = useCallback(
    (next: Partial<LibraryFilters>) => {
      navigate({
        search: (previous) => ({
          ...previous,
          ...blankToUndefined(next),
          // Changing what is shown invalidates a cursor taken over the old
          // filter, for the same reason changing the ordering does.
          cursor: undefined,
        }),
        // No history entry per keystroke, and none per dropdown change: the
        // back button should leave the library, not walk back through eight
        // intermediate filter states.
        replace: true,
      })
    },
    [navigate],
  )

  // Selection is component state, not a search parameter: a URL carrying
  // forty ids is not shareable in any useful sense, and it would make every
  // checkbox a navigation.
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set())

  const onToggle = useCallback((id: string) => {
    setSelected((previous) => {
      const next = new Set(previous)
      if (!next.delete(id)) next.add(id)
      return next
    })
  }, [])

  const onToggleAll = useCallback(() => {
    setSelected((previous) =>
      // Every row already selected means the press is a clear. Anything else
      // — none, or a partial selection — means select all, which matches what
      // the indeterminate box is offering.
      items.every((manga) => previous.has(manga.id)) && items.length > 0
        ? new Set()
        : new Set(items.map((manga) => manga.id)),
    )
  }, [items])

  return (
    <div className="flex h-dvh">
      <main className="flex min-w-0 flex-1 flex-col">
        <LibraryToolbar
          filters={{ q: search.q, status: search.status ?? '', source: search.source ?? '' }}
          sources={sources}
          statuses={statuses}
          shown={items.length}
          total={data.items.length}
          onChange={onFilterChange}
        />
        <div className="flex-1 overflow-y-auto">
          <LibraryTable
            items={items}
            sort={search.sort}
            dir={search.dir}
            selected={selected}
            onToggle={onToggle}
            onToggleAll={onToggleAll}
          />
        </div>
      </main>
      {/* The detail panel renders here. The parent stays mounted, so the
          virtualizer's scroll offset and the infinite query's accumulated
          pages survive opening it. */}
      <aside className="w-[420px] shrink-0 border-l">
        <Outlet />
      </aside>
    </div>
  )
}

/** The distinct values of a column, ordered for a dropdown. */
function distinct(values: string[]): string[] {
  return [...new Set(values)].sort((a, b) => a.localeCompare(b))
}

/** Drops a filter set back to its empty value, so the key leaves the URL. */
function blankToUndefined(next: Partial<LibraryFilters>): Partial<LibraryFilters> {
  return Object.fromEntries(
    Object.entries(next).map(([key, value]) => [key, value === '' ? undefined : value]),
  )
}

/**
 * Narrows the loaded page to what matches the filter bar.
 *
 * Client-side, over one page, for the same reason as the sort below. The text
 * comparison is case-insensitive through `toLocaleLowerCase`, because the
 * plain `toLowerCase` gets Turkish dotted and dotless I wrong.
 *
 * The two dropdowns match exactly: their options come from the data itself,
 * so a substring match would only let "ongoing" also select "not ongoing".
 */
function filterItems(
  items: MangaSummary[],
  filters: { q?: string | undefined; status?: string | undefined; source?: string | undefined },
): MangaSummary[] {
  const needle = filters.q?.trim().toLocaleLowerCase() ?? ''

  return items.filter((manga) => {
    if (filters.status !== undefined && filters.status !== '' && manga.status !== filters.status) {
      return false
    }
    if (
      filters.source !== undefined &&
      filters.source !== '' &&
      manga.source_name !== filters.source
    ) {
      return false
    }
    if (needle === '') return true
    return (
      manga.title.toLocaleLowerCase().includes(needle) ||
      manga.source_name.toLocaleLowerCase().includes(needle)
    )
  })
}

/**
 * Orders the loaded page.
 *
 * Client-side because the server returns one keyset page ordered by creation,
 * and re-sorting across pages would need the server to do it — which it does
 * not offer. So this orders what is on screen, and the ordering resets per
 * page rather than pretending to span the library.
 */
function sortItems(
  items: MangaSummary[],
  sort: 'title' | 'updated' | 'added' | 'chapters',
  dir: 'asc' | 'desc',
): MangaSummary[] {
  const compare = (a: MangaSummary, b: MangaSummary): number => {
    switch (sort) {
      case 'title':
        // `localeCompare` rather than `<`: the latter orders by code point, so
        // "Álbum" sorts after "Zebra" and a Spanish reader sees a list that
        // looks unsorted.
        return a.title.localeCompare(b.title)
      case 'updated':
        return a.updated_at.localeCompare(b.updated_at)
      case 'added':
        return a.created_at.localeCompare(b.created_at)
      case 'chapters':
        return a.chapter_count - b.chapter_count
    }
  }

  // Copied before sorting: `toSorted` leaves the query cache's array alone,
  // and mutating it would reorder what every other consumer of that cache
  // entry sees.
  return items.toSorted((a, b) => (dir === 'asc' ? compare(a, b) : -compare(a, b)))
}

/// Stable keys for a list that never reorders. An index would do, but only
/// because of a property of this list that nothing states — and the lint that
/// objects is right about every other list.
const SKELETON_ROWS = Array.from({ length: 12 }, (_, index) => `skeleton-${index}`)

function LibrarySkeleton() {
  return (
    <div className="flex flex-col">
      {SKELETON_ROWS.map((id) => (
        <div key={id} className="border-border h-row border-b px-cell flex items-center">
          <div className="bg-surface-raised h-3 w-1/3 animate-pulse rounded-sm" />
        </div>
      ))}
    </div>
  )
}

function LibraryError() {
  return (
    <div className="p-8 text-sm">
      <Trans>The library could not be loaded.</Trans>
    </div>
  )
}
