import { Trans } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import type { components } from '@/shared/api/schema'

type Manga = components['schemas']['MangaDto']

/**
 * One series in the library grid.
 *
 * A `Link` rather than a click handler: the detail panel is a route
 * (ADR-0017), so this has to be something a browser can open in a new tab and
 * a screen reader announces as a link. A `div` with an `onClick` is neither.
 */
export function MangaCard({ manga }: { manga: Manga }) {
  return (
    <Link
      to="/library/$mangaId"
      params={{ mangaId: manga.id }}
      // Search parameters are retained by the parent route's middleware, so
      // the current filter survives opening the panel (ADR-0017).
      search={(previous) => previous}
      className="group flex flex-col gap-2 rounded-lg outline-offset-2 focus-visible:outline-2"
      activeProps={{ 'aria-current': 'page' }}
    >
      <div className="bg-muted aspect-[2/3] overflow-hidden rounded-lg">
        {manga.cover_url ? (
          <img
            src={manga.cover_url}
            // The title is already rendered below, so repeating it here would
            // make a screen reader announce the series twice. The image is
            // decorative in the presence of its own caption.
            alt=""
            loading="lazy"
            decoding="async"
            className="size-full object-cover transition-transform group-hover:scale-105"
          />
        ) : (
          <div className="text-muted-foreground flex size-full items-center justify-center p-2 text-center text-xs">
            <Trans>No cover</Trans>
          </div>
        )}
      </div>

      <div className="min-w-0">
        <p className="truncate text-sm font-medium" title={manga.title}>
          {manga.title}
        </p>
        {manga.authors.length > 0 && (
          <p className="text-muted-foreground truncate text-xs">{manga.authors.join(', ')}</p>
        )}
      </div>
    </Link>
  )
}
