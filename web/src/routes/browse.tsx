import { Trans, useLingui } from '@lingui/react/macro'
import { useQuery, useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Link, Outlet } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { z } from 'zod'
import { CatalogFilters } from '@/features/browse/components/CatalogFilters'
import { CatalogGrid } from '@/features/browse/components/CatalogGrid'
import { parseFilters, parseState, toValues } from '@/features/browse/lib/filters'
import { activeSource } from '@/features/browse/lib/source'
import { useAddToLibrary } from '@/features/sources/api/mutations'
import { catalogQuery, sourceFiltersQuery, sourceListQuery } from '@/features/sources/api/queries'
import { isProblem, ProblemType, problemMessage } from '@/shared/api/problem'

/**
 * Browse a source's catalog.
 *
 * The source is a search parameter rather than a path segment: it is a filter
 * over one screen, not a different screen, and it keeps `/browse` meaningful
 * on its own — which is what the sidebar links to before any source is picked.
 */
export const Route = createFileRoute('/browse')({
  validateSearch: z.object({
    source: z.string().optional(),
    q: z.string().optional(),
    /**
     * The source's own filters, as the JSON array the API takes.
     *
     * Kept as the encoded string rather than a parsed object: it goes to the
     * server verbatim, it is what the query is keyed on, and parsing it into
     * the URL schema would mean re-encoding it identically on every render to
     * avoid rewriting the address bar.
     */
    filters: z.string().optional(),
    cursor: z.string().optional(),
  }),

  // The installed sources are needed before anything can be browsed, and they
  // are what the picker renders.
  loader: ({ context }) => context.queryClient.ensureQueryData(sourceListQuery()),

  component: BrowseScreen,
  errorComponent: BrowseError,
})

function BrowseScreen() {
  const { source, q, filters, cursor } = Route.useSearch()
  const { data: sources } = useSuspenseQuery(sourceListQuery())

  // The detail panel resolves this the same way, through the same function —
  // see `activeSource`.
  const active = activeSource(source, sources)

  if (sources.length === 0) return <NoSources />

  return (
    <div className="flex h-full">
      <main className="flex min-w-0 flex-1 flex-col">
        <BrowseToolbar sources={sources} active={active} q={q} />
        <div className="min-h-0 flex-1 overflow-y-auto">
          {active === undefined ? (
            <NoSources />
          ) : (
            <Catalog sourceId={active} q={q} filters={filters} cursor={cursor} />
          )}
        </div>
      </main>

      {/* The detail panel renders here. The parent stays mounted, so the
          grid's scroll position and the loaded page survive opening it
          (ADR-0017). */}
      <aside className="border-border w-[420px] shrink-0 overflow-y-auto border-l">
        <Outlet />
      </aside>
    </div>
  )
}

function Catalog({
  sourceId,
  q,
  filters,
  cursor,
}: {
  sourceId: string
  q: string | undefined
  filters: string | undefined
  cursor: string | undefined
}) {
  const navigate = Route.useNavigate()

  // The declaration. `useQuery`, not suspense: a source that will not answer
  // must not take the whole screen down, and the catalog below renders its own
  // failure.
  const { data: declared } = useQuery(sourceFiltersQuery(sourceId))
  const available = parseFilters(declared)
  const state = parseState(filters)
  // `useQuery`, not `useSuspenseQuery`: this request leaves the deployment, so
  // it fails in ways the others do not and the screen has to render that
  // rather than throw it to a route-level boundary that hides the toolbar.
  const { data, error, isPending } = useQuery(catalogQuery(sourceId, q, filters, cursor))
  const add = useAddToLibrary(sourceId)

  const adding = new Set(add.isPending && add.variables ? [add.variables] : [])

  const filterBar = (
    <CatalogFilters
      filters={available}
      state={state}
      onChange={(next) => {
        const values = toValues(available, next)
        void navigate({
          search: (previous) => ({
            ...previous,
            filters: values.length === 0 ? undefined : JSON.stringify(values),
            // A cursor belongs to the listing it came from.
            cursor: undefined,
          }),
        })
      }}
    />
  )

  // Rendered above whatever the catalog is doing, so changing a filter stays
  // possible while the result is loading or failing — which is exactly when
  // someone wants to.
  if (error) {
    return (
      <>
        {filterBar}
        <CatalogError error={error} />
      </>
    )
  }
  if (isPending) {
    return (
      <>
        {filterBar}
        <CatalogSkeleton />
      </>
    )
  }

  return (
    <>
      {filterBar}
      <CatalogGrid
        items={data.items}
        adding={adding}
        onAdd={(externalKey) => add.mutate(externalKey)}
      />
      {data.next_cursor !== null && data.next_cursor !== undefined && (
        <div className="flex justify-center p-4">
          <Link
            to="/browse"
            search={(previous) => ({ ...previous, cursor: data.next_cursor ?? undefined })}
            className="border-border hover:bg-surface-raised rounded-sm border px-3 py-1.5 text-sm"
          >
            <Trans>Next page</Trans>
          </Link>
        </div>
      )}
    </>
  )
}

function BrowseToolbar({
  sources,
  active,
  q,
}: {
  sources: { id: string; name: string }[]
  active: string | undefined
  q: string | undefined
}) {
  const { t } = useLingui()
  const navigate = Route.useNavigate()
  const [draft, setDraft] = useState(q ?? '')

  // The URL wins, so the back button and a pasted link both take effect.
  useEffect(() => setDraft(q ?? ''), [q])

  return (
    <form
      className="border-border px-cell flex h-row shrink-0 items-center gap-2 border-b"
      onSubmit={(event) => {
        event.preventDefault()
        void navigate({
          search: (previous) => ({
            ...previous,
            q: draft.trim() === '' ? undefined : draft.trim(),
            // A cursor belongs to the listing it came from.
            cursor: undefined,
          }),
        })
      }}
    >
      <label className="text-muted-foreground flex items-center gap-1 text-xs">
        <Trans>Source</Trans>
        <select
          value={active ?? ''}
          onChange={(event) =>
            void navigate({
              search: (previous) => ({
                ...previous,
                source: event.target.value,
                // A filter belongs to the source that declared it. Carrying
                // one across would send another source ids it has never heard
                // of — refused at best, silently unmatched at worst.
                filters: undefined,
                cursor: undefined,
              }),
            })
          }
          className="text-foreground bg-surface border-border h-7 rounded-sm border px-1 text-xs"
        >
          {sources.map((source) => (
            <option key={source.id} value={source.id}>
              {source.name}
            </option>
          ))}
        </select>
      </label>

      {/*
        A form rather than a debounced box, unlike the library's filter: each
        keystroke here is a request to a third-party site, and typing eight
        characters must not be eight requests to someone else's server.
      */}
      <input
        type="search"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        placeholder={t`Search this source`}
        aria-label={t`Search this source`}
        className="border-border bg-surface h-7 w-64 rounded-sm border px-2 text-sm"
      />
      <button
        type="submit"
        className="border-border hover:bg-surface-raised h-7 rounded-sm border px-2 text-xs"
      >
        <Trans>Search</Trans>
      </button>
    </form>
  )
}

/**
 * A catalog request that failed.
 *
 * Rendered in place rather than thrown to the route boundary: the toolbar has
 * to stay usable, because switching source or clearing the search is exactly
 * what a reader will want to do next.
 *
 * A 404 and a 502 are different situations and say so. "Could not be reached"
 * on a source that no longer exists sends the reader to check their network
 * over something a refresh will never fix — the source was uninstalled, or the
 * link is stale.
 */
function CatalogError({ error }: { error: unknown }) {
  const gone = isProblem(error) && error.type === ProblemType.NotFound
  const detail = problemMessage(error)

  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        {gone ? (
          <Trans>That source is no longer installed</Trans>
        ) : (
          <Trans>This source could not be reached</Trans>
        )}
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        {/* `problemMessage` returns null when the server said nothing
            specific, so the fallback is a translated sentence rather than an
            empty paragraph. */}
        {detail ?? <Trans>The request failed before it reached the source.</Trans>}
      </p>
    </div>
  )
}

function NoSources() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>No sources installed</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>Add a repository in Settings, then install a source from it.</Trans>
      </p>
      <Link to="/settings" className="text-accent text-sm">
        <Trans>Go to Settings</Trans>
      </Link>
    </div>
  )
}

/// Stable keys for a fixed-length placeholder grid.
const SKELETON_CARDS = Array.from({ length: 12 }, (_, index) => `skeleton-${index}`)

function CatalogSkeleton() {
  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-4 p-4">
      {SKELETON_CARDS.map((id) => (
        <li key={id} className="bg-surface-raised aspect-[2/3] animate-pulse rounded-lg" />
      ))}
    </ul>
  )
}

function BrowseError() {
  return (
    <div className="p-8 text-sm">
      <Trans>Browse could not be loaded.</Trans>
    </div>
  )
}
