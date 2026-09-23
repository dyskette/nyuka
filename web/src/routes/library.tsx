import { Trans } from '@lingui/react/macro'
import { useSuspenseInfiniteQuery, useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Outlet, retainSearchParams } from '@tanstack/react-router'
import { useCallback, useMemo, useState } from 'react'
import { z } from 'zod'
import { libraryInfiniteQuery } from '@/features/library/api/queries'
import { LibraryTable } from '@/features/library/components/LibraryTable'
import { type LibraryFilters, LibraryToolbar } from '@/features/library/components/LibraryToolbar'
import { sourceListQuery } from '@/features/sources/api/queries'

/**
 * The publication statuses the server accepts as a filter.
 *
 * A fixed list, not the values present on the loaded page: a status nobody in
 * the library has is still a status worth being able to filter to zero, and
 * deriving the list from the page would hide it.
 */
const STATUSES = ['unknown', 'ongoing', 'completed', 'cancelled', 'hiatus'] as const

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
  // Everything the server uses. Sort and direction are here now: they change
  // what the server returns, so a loader that ignored them would prime the
  // cache with a differently ordered page than the component asks for.
  //
  // The cursor is absent: pages accumulate in one infinite query, and a
  // cursor in the URL would name a position in a list that no longer starts
  // where it did.
  loaderDeps: ({ search }) => ({
    q: search.q,
    status: search.status,
    source_id: search.source,
    sort: search.sort,
    dir: search.dir,
  }),

  // Primes the cache so the table has data on first paint. The loader returns
  // nothing the component reads: components always read through hooks, so
  // there is one source of truth for the data and one for its loading state
  // (ADR-0008).
  loader: ({ context, deps }) =>
    Promise.all([
      context.queryClient.ensureInfiniteQueryData(libraryInfiniteQuery(deps)),
      context.queryClient.ensureQueryData(sourceListQuery()),
    ]),

  component: LibraryLayout,
  pendingComponent: LibrarySkeleton,
  errorComponent: LibraryError,
})

function LibraryLayout() {
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  // The server orders and filters. There is nothing left to do here, and
  // anything done here would apply to one page rather than to the library.
  const { data, fetchNextPage, hasNextPage, isFetchingNextPage } = useSuspenseInfiniteQuery(
    libraryInfiniteQuery({
      q: search.q,
      status: search.status,
      source_id: search.source,
      sort: search.sort,
      dir: search.dir,
    }),
  )

  // Flattened once per data change rather than on every render: the
  // virtualizer indexes into this array, and a new array identity each render
  // would make every memo below it recompute.
  const items = useMemo(() => data.pages.flatMap((page) => page.items), [data.pages])

  // Offered from what is on the page rather than from a fixed list: a source
  // the reader has not installed is not a filter worth showing, and an
  // enumeration of every possible status would offer several that match
  // nothing.
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

  // The options come from the installed sources, not from the loaded page.
  // Deriving them from the page was fine while filtering happened here; now
  // that the server filters it is circular — choosing a source would leave
  // that source as the only option, with no way back.
  const { data: sources } = useSuspenseQuery(sourceListQuery())

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
          statuses={STATUSES}
          shown={items.length}
          hasMore={hasNextPage}
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
            onReachEnd={fetchNextPage}
            loadingMore={isFetchingNextPage}
            hasMore={hasNextPage}
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

/** Drops a filter set back to its empty value, so the key leaves the URL. */
function blankToUndefined(next: Partial<LibraryFilters>): Partial<LibraryFilters> {
  return Object.fromEntries(
    Object.entries(next).map(([key, value]) => [key, value === '' ? undefined : value]),
  )
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
