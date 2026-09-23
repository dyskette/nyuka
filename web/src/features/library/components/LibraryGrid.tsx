import { Trans } from '@lingui/react/macro'
import type { components } from '@/shared/api/schema'
import { MangaCard } from './MangaCard'

type Manga = components['schemas']['MangaDto']

/**
 * The library grid.
 *
 * Presentational: it takes what it renders rather than fetching, so the route
 * decides what is loaded and this can be exercised without a query client
 * (ADR-0008).
 */
export function LibraryGrid({ items }: { items: Manga[] }) {
  if (items.length === 0) return <EmptyLibrary />

  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-4 p-4">
      {items.map((manga) => (
        <li key={manga.id}>
          <MangaCard manga={manga} />
        </li>
      ))}
    </ul>
  )
}

/**
 * Says what to do next rather than only that there is nothing.
 *
 * An empty library on a fresh install is the expected state, not an error, and
 * it is the one moment where the next action is genuinely unobvious.
 */
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
