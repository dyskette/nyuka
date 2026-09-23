import { Trans } from '@lingui/react/macro'
import { useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Outlet, retainSearchParams } from '@tanstack/react-router'
import { useMemo } from 'react'
import { z } from 'zod'
import { libraryListQuery } from '@/features/library/api/queries'
import { LibraryGrid } from '@/features/library/components/LibraryGrid'
import type { components } from '@/shared/api/schema'

type Manga = components['schemas']['MangaDto']

/**
 * The library master list, and the parent of the detail panel (ADR-0017).
 *
 * Search parameters are validated here, on the parent, so the detail route
 * inherits them and `retainSearchParams` can preserve them across opening and
 * closing the panel.
 */
const searchSchema = z.object({
  q: z.string().optional(),
  sort: z.enum(['title', 'updated', 'added']).default('updated'),
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
  search: { middlewares: [retainSearchParams(['q', 'sort', 'dir', 'cursor'])] },

  // Declared before `loader`, because that is what `deps` is inferred from.
  //
  // Only the cursor affects what is fetched. Sort and direction are applied to
  // what is already loaded, so listing them here would refetch the same page
  // every time the user changed the ordering.
  loaderDeps: ({ search }) => ({ cursor: search.cursor }),

  // Primes the cache so the grid has data on first paint. The loader returns
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
  const { data } = useSuspenseQuery(libraryListQuery(search.cursor))

  const items = useMemo(
    () => sortItems(data.items, search.sort, search.dir),
    [data.items, search.sort, search.dir],
  )

  return (
    <div className="flex h-dvh">
      <main className="flex-1 overflow-y-auto">
        <LibraryGrid items={items} />
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

/**
 * Orders the loaded page.
 *
 * Client-side because the server returns one keyset page ordered by creation,
 * and re-sorting across pages would need the server to do it — which it does
 * not offer. So this orders what is on screen, and the ordering resets per
 * page rather than pretending to span the library.
 */
function sortItems(
  items: Manga[],
  sort: 'title' | 'updated' | 'added',
  dir: 'asc' | 'desc',
): Manga[] {
  const compare = (a: Manga, b: Manga): number => {
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
const SKELETON_CARDS = Array.from({ length: 12 }, (_, index) => `skeleton-${index}`)

function LibrarySkeleton() {
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-4 p-4">
      {SKELETON_CARDS.map((id) => (
        <div key={id} className="bg-muted aspect-[2/3] animate-pulse rounded-lg" />
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
