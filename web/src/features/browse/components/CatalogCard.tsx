import { Trans, useLingui } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import type { components } from '@/shared/api/schema'

type CatalogItem = components['schemas']['CatalogItemDto']

export interface CatalogCardProps {
  item: CatalogItem
  /** True while this entry's add request is in flight. */
  adding?: boolean
  onAdd: (externalKey: string) => void
}

/**
 * One result in a source's catalog.
 *
 * A catalog entry has no local identity until it is added, so this is not a
 * link the way a library card is — `manga_id` is what decides. Present means
 * the series is already in the library and the card opens it; absent means the
 * only thing to do is add it.
 */
export function CatalogCard({ item, adding = false, onAdd }: CatalogCardProps) {
  return (
    <div className="group flex h-full flex-col gap-2">
      {/*
        The cover and the title open the panel, which is where a reader decides
        whether to add. The action below stays a separate control: adding
        without looking is the common case, and putting both on one element
        would mean choosing which one a click means.
      */}
      <Link
        to="/browse/$key"
        params={{ key: item.external_key }}
        search={(previous) => previous}
        className="flex flex-col gap-2"
        activeProps={{ 'aria-current': 'page' }}
      >
        <div className="bg-surface-raised relative aspect-[2/3] overflow-hidden rounded-lg">
          <Cover item={item} />
          <InLibraryBadge mangaId={item.manga_id} />
        </div>

        {/*
          Two lines reserved whether or not the title needs them. `line-clamp-2`
          alone left one-line titles a line short, so the action below sat at a
          different height on every card. `min-h-10` is two lines of `text-sm`,
          whose line height is `1.25rem`.
        */}
        <span className="line-clamp-2 min-h-10 text-sm font-medium" title={item.title}>
          {item.title}
        </span>
      </Link>

      <div className="mt-auto">
        <Action item={item} adding={adding} onAdd={onAdd} />
      </div>
    </div>
  )
}

function Cover({ item }: { item: CatalogItem }) {
  if (item.cover_url === null || item.cover_url === undefined) {
    return (
      <div className="text-muted-foreground flex size-full items-center justify-center text-xs">
        <Trans>No cover</Trans>
      </div>
    )
  }

  return (
    <img
      src={item.cover_url}
      // The title is rendered below, so alt text repeating it would make a
      // screen reader announce the series twice.
      alt=""
      loading="lazy"
      decoding="async"
      // A source's cover host is a third party. `no-referrer` keeps it from
      // learning which server its images are being read from.
      referrerPolicy="no-referrer"
      className="size-full object-cover transition-transform group-hover:scale-105"
    />
  )
}

/** The mockup's "✓ Library" mark. */
function InLibraryBadge({ mangaId }: { mangaId: string | null | undefined }) {
  if (mangaId === null || mangaId === undefined) return null

  return (
    <span className="bg-surface/90 absolute top-1 right-1 flex items-center gap-1 rounded-sm px-1.5 py-0.5 text-xs">
      {/* The check is decorative — the word beside it carries the meaning, so
          the badge does not depend on recognising a glyph (ADR-0016). */}
      <span aria-hidden="true">✓</span>
      <Trans>Library</Trans>
    </span>
  )
}

/**
 * The one box both actions wear.
 *
 * A card offers exactly one of "Add" and "Open in library", so the two sit in
 * the same place on neighbouring cards. Declared once because a grid where
 * they differ reads as one card being broken rather than as two states.
 */
const ACTION_BOX =
  'border-border hover:bg-accent-soft block w-full rounded-sm border px-2 py-1 text-center text-xs'

function Action({
  item,
  adding,
  onAdd,
}: {
  item: CatalogItem
  adding: boolean
  onAdd: (externalKey: string) => void
}) {
  const { t } = useLingui()

  if (item.manga_id !== null && item.manga_id !== undefined) {
    return (
      <Link
        to="/library/$mangaId"
        params={{ mangaId: item.manga_id }}
        search={{ tab: 'overview' }}
        className={ACTION_BOX}
      >
        <Trans>Open in library</Trans>
      </Link>
    )
  }

  return (
    <button
      type="button"
      onClick={() => onAdd(item.external_key)}
      disabled={adding}
      // The title, not "add": a screen reader's list of buttons would
      // otherwise be forty identical entries.
      aria-label={t`Add ${item.title} to your library`}
      className={`${ACTION_BOX} disabled:opacity-50`}
    >
      {adding ? <Trans>Adding…</Trans> : <Trans>Add</Trans>}
    </button>
  )
}
