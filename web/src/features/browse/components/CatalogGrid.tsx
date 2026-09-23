import { Trans } from '@lingui/react/macro'
import type { components } from '@/shared/api/schema'
import { CatalogCard } from './CatalogCard'

type CatalogItem = components['schemas']['CatalogItemDto']

export interface CatalogGridProps {
  items: CatalogItem[]
  /** External keys whose add request is in flight. */
  adding?: ReadonlySet<string>
  onAdd: (externalKey: string) => void
}

/**
 * The cover grid.
 *
 * This is the Browse shape — a source's catalog, where the cover is the
 * primary way a reader recognises a series. The library uses a dense table
 * instead, because there the useful columns are progress and recency, and a
 * grid of covers hides both.
 *
 * Presentational: it takes what it renders rather than fetching, so the route
 * decides what is loaded and this can be exercised without a query client
 * (ADR-0008).
 */
export function CatalogGrid({ items, adding, onAdd }: CatalogGridProps) {
  if (items.length === 0) return <EmptyCatalog />

  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-4 p-4">
      {items.map((item) => (
        // Keyed on the source's own key, not an index: adding a series
        // refetches the page, and an index key would re-use one card's
        // in-flight state for whatever moved into its position.
        <li key={item.external_key}>
          <CatalogCard item={item} adding={adding?.has(item.external_key) ?? false} onAdd={onAdd} />
        </li>
      ))}
    </ul>
  )
}

function EmptyCatalog() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>Nothing here</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>This source returned no results. Try a different search.</Trans>
      </p>
    </div>
  )
}
