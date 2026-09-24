import { Trans, useLingui } from '@lingui/react/macro'
import { useQuery, useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Link } from '@tanstack/react-router'
import { activeSource } from '@/features/browse/lib/source'
import { useAddToLibrary } from '@/features/sources/api/mutations'
import {
  catalogChaptersQuery,
  catalogItemQuery,
  sourceListQuery,
} from '@/features/sources/api/queries'
import { problemMessage } from '@/shared/api/problem'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'

type CatalogItem = components['schemas']['CatalogItemDto']
type Chapter = components['schemas']['ChapterDto']

/**
 * What a source says about one series, before it is in the library.
 *
 * A nested route for the reasons ADR-0017 gives for the library's panel: the
 * parent stays mounted, so the catalog grid's scroll position and the loaded
 * page survive opening this, and the panel gets its own loader and boundaries
 * rather than hand-rolled ones inside a component.
 *
 * # Everything here comes from a third party
 *
 * Unlike the library panel, none of this has been through the database. The
 * description, the tags and the chapter titles are whatever a source returned
 * on this request, so nothing is trusted to be present or well-formed.
 */
export const Route = createFileRoute('/browse/$key')({
  // No loader. The source id comes from the parent's search parameters, which
  // a loader cannot see; the component reads them and resolves the same
  // fallback the catalog grid does.
  component: CatalogPanel,
  errorComponent: PanelError,
})

function CatalogPanel() {
  const { key } = Route.useParams()
  const { source } = Route.useSearch()
  // Already loaded: the parent route's loader ensures it before this renders,
  // so this reads the cache rather than opening a second request.
  const { data: sources } = useSuspenseQuery(sourceListQuery())

  const active = activeSource(source, sources)

  // Only reachable with nothing installed at all, which the screen behind
  // this panel is already saying.
  if (active === undefined) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>Install a source to see this series.</Trans>
      </p>
    )
  }

  return <Details sourceId={active} externalKey={key} />
}

function Details({ sourceId, externalKey }: { sourceId: string; externalKey: string }) {
  const item = useQuery(catalogItemQuery(sourceId, externalKey))
  const add = useAddToLibrary(sourceId)

  if (item.isPending) return <PanelSkeleton />
  if (item.error) {
    return (
      <p className="text-danger p-panel text-sm" role="alert">
        {problemMessage(item.error) ?? <Trans>This series could not be read.</Trans>}
      </p>
    )
  }

  return (
    <div className="flex h-full flex-col">
      <Header item={item.data} adding={add.isPending} onAdd={() => add.mutate(externalKey)} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Chapters sourceId={sourceId} externalKey={externalKey} />
      </div>
    </div>
  )
}

function Header({
  item,
  adding,
  onAdd,
}: {
  item: CatalogItem
  adding: boolean
  onAdd: () => void
}) {
  const { t } = useLingui()

  return (
    <header className="border-border p-panel flex shrink-0 flex-col gap-3 border-b">
      <div className="flex gap-3">
        {item.cover_url !== null && item.cover_url !== undefined ? (
          <img
            src={item.cover_url}
            alt=""
            // A source's cover host is a third party and does not need the
            // address of the server displaying its images.
            referrerPolicy="no-referrer"
            className="bg-surface-raised h-28 w-20 shrink-0 rounded object-cover"
          />
        ) : (
          <div className="bg-surface-raised h-28 w-20 shrink-0 rounded" aria-hidden="true" />
        )}

        <div className="min-w-0 flex-1">
          <div className="flex items-start gap-2">
            <h2 className="min-w-0 flex-1 text-base font-semibold">{item.title}</h2>
            <Link
              to="/browse"
              search={(previous) => previous}
              aria-label={t`Close`}
              className="text-muted-foreground hover:text-foreground shrink-0 px-1"
            >
              ×
            </Link>
          </div>

          <p className="text-muted-foreground mt-0.5 truncate text-xs">
            {[item.authors.join(', '), item.status].filter(Boolean).join(' · ')}
          </p>

          {item.tags.length > 0 && (
            <ul className="mt-2 flex flex-wrap gap-1">
              {item.tags.slice(0, 8).map((tag) => (
                <li
                  key={tag}
                  className="bg-surface-raised text-muted-foreground rounded-sm px-1.5 py-0.5 text-xs"
                >
                  {tag}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {/*
        `manga_id` is the server's answer to "is this already in". The panel
        exists so a reader can decide before adding, so this is the decision
        it leads to.
      */}
      {item.manga_id !== null && item.manga_id !== undefined ? (
        <Link
          to="/library/$mangaId"
          params={{ mangaId: item.manga_id }}
          search={{ tab: 'chapters' }}
          className="border-border hover:bg-surface-raised rounded-sm border px-3 py-1.5 text-center text-sm"
        >
          <Trans>Open in library</Trans>
        </Link>
      ) : (
        <button
          type="button"
          onClick={onAdd}
          disabled={adding}
          className="bg-accent text-accent-foreground rounded-sm px-3 py-1.5 text-sm disabled:opacity-50"
        >
          {adding ? <Trans>Adding…</Trans> : <Trans>Add to library</Trans>}
        </button>
      )}

      {item.description !== null && item.description !== undefined && (
        <p className="text-muted-foreground whitespace-pre-line text-sm">{item.description}</p>
      )}

      {item.url !== null && item.url !== undefined && (
        <a href={item.url} target="_blank" rel="noreferrer" className="text-accent text-xs">
          <Trans>Open at the source</Trans>
        </a>
      )}
    </header>
  )
}

/**
 * The chapters a source lists.
 *
 * No download action. These chapters have no local id — nothing here is in the
 * library yet — and `POST /downloads` addresses one by its id. Adding the
 * series is what creates them, which is why that is the only action on this
 * panel.
 */
function Chapters({ sourceId, externalKey }: { sourceId: string; externalKey: string }) {
  const { data, error, isPending } = useQuery(catalogChaptersQuery(sourceId, externalKey))

  if (isPending) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>Reading the chapter list…</Trans>
      </p>
    )
  }

  if (error) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>The chapter list could not be read.</Trans>
      </p>
    )
  }

  if (data.length === 0) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>This source lists no chapters.</Trans>
      </p>
    )
  }

  return (
    <ul className="flex flex-col">
      {/* Newest first, as the panel for a library series shows them. */}
      {data.toReversed().map((chapter) => (
        <ChapterRow key={chapter.external_key} chapter={chapter} />
      ))}
    </ul>
  )
}

function ChapterRow({ chapter }: { chapter: Chapter }) {
  return (
    <li className="border-border px-cell h-row flex items-center gap-2 border-b text-sm">
      <span className="tabular text-muted-foreground w-10 shrink-0 text-xs">
        {chapter.number != null ? String(Number(chapter.number)) : '—'}
      </span>
      <span className="min-w-0 flex-1 truncate">
        {chapter.title !== null && chapter.title !== undefined && chapter.title !== ''
          ? chapter.title
          : chapter.external_key}
      </span>
      {chapter.published_at !== null && chapter.published_at !== undefined && (
        <time dateTime={chapter.published_at} className="text-muted-foreground shrink-0 text-xs">
          {relativeTime(chapter.published_at)}
        </time>
      )}
    </li>
  )
}

/// Stable keys for a fixed-length placeholder list.
const SKELETON_ROWS = Array.from({ length: 6 }, (_, index) => `skeleton-${index}`)

function PanelSkeleton() {
  return (
    <div className="p-panel flex flex-col gap-2">
      <div className="bg-surface-raised h-28 w-20 animate-pulse rounded" />
      {SKELETON_ROWS.map((id) => (
        <div key={id} className="bg-surface-raised h-3 w-full animate-pulse rounded-sm" />
      ))}
    </div>
  )
}

function PanelError() {
  return (
    <div className="p-panel text-sm">
      <Trans>This series could not be loaded.</Trans>
    </div>
  )
}
