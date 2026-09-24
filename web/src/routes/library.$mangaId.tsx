import { Trans, useLingui } from '@lingui/react/macro'
import { useQuery, useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Link } from '@tanstack/react-router'
import { z } from 'zod'
import { useRemoveFromLibrary, useRequestDownload } from '@/features/library/api/mutations'
import { mangaChaptersQuery, mangaDetailQuery } from '@/features/library/api/queries'
import { ChapterList } from '@/features/library/components/ChapterList'
import { RemoveSeries } from '@/features/library/components/RemoveSeries'
import { problemMessage } from '@/shared/api/problem'
import type { components } from '@/shared/api/schema'

type Manga = components['schemas']['MangaDto']

const TABS = ['overview', 'chapters'] as const
type Tab = (typeof TABS)[number]

/**
 * The detail panel (ADR-0017).
 *
 * Being a route is the point: `loader`, `pendingComponent` and
 * `errorComponent` come from the router instead of being hand-rolled inside a
 * component, which is what a `?selected=` search parameter would have
 * required. The parent stays mounted, so the list's scroll position survives
 * opening and closing this.
 */
export const Route = createFileRoute('/library/$mangaId')({
  // The active tab is a search param so a link can open the panel straight to
  // the chapter list, and the back button closes a tab change rather than the
  // whole panel.
  validateSearch: z.object({ tab: z.enum(TABS).default('overview') }),

  // Both queries are primed, not just the one the default tab renders:
  // switching tabs is not a navigation, so a lazily loaded chapter list would
  // show a spinner on a panel that is already open and looks ready.
  loader: ({ context, params }) =>
    Promise.all([
      context.queryClient.ensureQueryData(mangaDetailQuery(params.mangaId)),
      context.queryClient.ensureQueryData(mangaChaptersQuery(params.mangaId)),
    ]),

  component: MangaPanel,
  pendingComponent: PanelSkeleton,
  errorComponent: PanelError,
})

function MangaPanel() {
  const { mangaId } = Route.useParams()
  const { tab } = Route.useSearch()
  const { data: manga } = useSuspenseQuery(mangaDetailQuery(mangaId))

  return (
    <div className="flex h-full flex-col">
      <PanelHeader manga={manga} />
      <Tabs active={tab} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        {tab === 'overview' ? <Overview manga={manga} /> : <Chapters mangaId={mangaId} />}
      </div>
    </div>
  )
}

function PanelHeader({ manga }: { manga: Manga }) {
  const { t } = useLingui()
  const navigate = Route.useNavigate()
  const remove = useRemoveFromLibrary()

  return (
    <header className="border-border p-panel flex shrink-0 gap-3 border-b">
      {/* A placeholder block rather than an <img> with no src: a broken image
          icon says the cover failed to load, which is a different claim from
          the source never having given one. */}
      {manga.cover_url !== null && manga.cover_url !== undefined ? (
        <img
          src={manga.cover_url}
          alt=""
          className="bg-surface-raised h-28 w-20 shrink-0 rounded object-cover"
        />
      ) : (
        <div className="bg-surface-raised h-28 w-20 shrink-0 rounded" aria-hidden="true" />
      )}

      <div className="min-w-0 flex-1">
        <div className="flex items-start gap-2">
          <h2 className="min-w-0 flex-1 text-base font-semibold">{manga.title}</h2>
          {/* Closing the panel is a navigation to the parent, so it keeps the
              list's filters and the back button behaves. */}
          <Link
            to="/library"
            search={(previous) => previous}
            aria-label={t`Close`}
            className="text-muted-foreground hover:text-foreground shrink-0 px-1"
          >
            ×
          </Link>
        </div>

        <p className="text-muted-foreground mt-0.5 truncate text-xs">
          {[manga.authors.join(', '), manga.status].filter(Boolean).join(' · ')}
        </p>

        {manga.tags.length > 0 && (
          <ul className="mt-2 flex flex-wrap gap-1">
            {manga.tags.map((tag) => (
              <li
                key={tag}
                className="bg-surface-raised text-muted-foreground rounded-sm px-1.5 py-0.5 text-xs"
              >
                {tag}
              </li>
            ))}
          </ul>
        )}

        <div className="mt-2">
          <RemoveSeries
            title={manga.title}
            removing={remove.isPending}
            error={
              remove.isError ? (problemMessage(remove.error) ?? t`That did not work.`) : undefined
            }
            onRemove={(files) =>
              remove.mutate(
                { mangaId: manga.id, files },
                // The panel is showing a series that no longer exists, so it
                // closes onto the list rather than onto its own 404.
                { onSuccess: () => void navigate({ to: '/library', search: (p) => p }) },
              )
            }
          />
        </div>
      </div>
    </header>
  )
}

/**
 * The panel's tabs.
 *
 * Links rather than buttons with state: the tab is in the URL, so each one
 * has a real href and the browser's own history does the work. `aria-current`
 * is what tells a screen reader which is active — the accent underline is not
 * available to it.
 */
function Tabs({ active }: { active: Tab }) {
  return (
    <nav className="border-border px-panel flex shrink-0 gap-3 border-b text-sm">
      <TabLink tab="overview" active={active}>
        <Trans>Overview</Trans>
      </TabLink>
      <TabLink tab="chapters" active={active}>
        <Trans>Chapters</Trans>
      </TabLink>
    </nav>
  )
}

function TabLink({ tab, active, children }: { tab: Tab; active: Tab; children: React.ReactNode }) {
  const current = tab === active
  return (
    <Link
      to="."
      search={(previous) => ({ ...previous, tab })}
      {...(current ? { 'aria-current': 'page' } : {})}
      className={`-mb-px border-b-2 py-2 ${
        current ? 'border-accent text-foreground' : 'text-muted-foreground border-transparent'
      }`}
    >
      {children}
    </Link>
  )
}

function Overview({ manga }: { manga: Manga }) {
  return (
    <div className="p-panel flex flex-col gap-3 text-sm">
      {manga.description !== null && manga.description !== undefined ? (
        <p className="whitespace-pre-line">{manga.description}</p>
      ) : (
        <p className="text-muted-foreground">
          <Trans>This source gave no description.</Trans>
        </p>
      )}

      {manga.url !== null && manga.url !== undefined && (
        <a
          href={manga.url}
          target="_blank"
          // `noreferrer` as well as `noopener`: the target page should not
          // learn which library this came from.
          rel="noreferrer"
          className="text-accent text-xs"
        >
          <Trans>Open at the source</Trans>
        </a>
      )}
    </div>
  )
}

function Chapters({ mangaId }: { mangaId: string }) {
  const { data } = useQuery(mangaChaptersQuery(mangaId))
  const download = useRequestDownload(mangaId)

  // `variables` is the chapter id currently in flight, so the row it belongs
  // to shows "Queued" without a second piece of state to keep in step.
  const pending = new Set(download.isPending ? [download.variables] : [])

  return (
    <ChapterList
      chapters={data?.items ?? []}
      pending={pending}
      onDownload={(chapterId) => download.mutate(chapterId)}
    />
  )
}

/// Stable keys for a fixed-length placeholder list.
const SKELETON_ROWS = Array.from({ length: 8 }, (_, index) => `skeleton-${index}`)

function PanelSkeleton() {
  // Appears after a 150ms delay so fast loads never flash (spec-design.md).
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
