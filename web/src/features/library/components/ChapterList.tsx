import { Trans } from '@lingui/react/macro'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'

type ChapterSummary = components['schemas']['ChapterSummaryDto']

export interface ChapterListProps {
  chapters: ChapterSummary[]
  /** Chapters with a download job in flight, by chapter id. */
  pending?: ReadonlySet<string>
  onDownload: (chapterId: string) => void
}

/**
 * A series' chapters, newest first, with what the reader can do about each.
 *
 * Reversed from the API's order: the server pages ascending by number so a
 * cursor stays stable as chapters are added, and a reader opening a series
 * wants the newest. Reversing the loaded page rather than asking the server
 * keeps the pagination contract intact.
 */
export function ChapterList({ chapters, pending, onDownload }: ChapterListProps) {
  if (chapters.length === 0) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>No chapters yet. Refresh the series to fetch its chapter list.</Trans>
      </p>
    )
  }

  return (
    <ul className="flex flex-col">
      {chapters.toReversed().map((chapter) => (
        <ChapterRow
          key={chapter.id}
          chapter={chapter}
          pending={pending?.has(chapter.id) ?? false}
          onDownload={onDownload}
        />
      ))}
    </ul>
  )
}

function ChapterRow({
  chapter,
  pending,
  onDownload,
}: {
  chapter: ChapterSummary
  pending: boolean
  onDownload: (chapterId: string) => void
}) {
  return (
    <li className="border-border hover:bg-surface-raised px-cell flex h-row items-center gap-2 border-b text-sm">
      <span className="tabular text-muted-foreground w-10 shrink-0 text-xs">
        {formatNumber(chapter.number)}
      </span>
      <span className="min-w-0 flex-1 truncate">{chapterLabel(chapter)}</span>
      {chapter.published_at !== null && chapter.published_at !== undefined && (
        <time dateTime={chapter.published_at} className="text-muted-foreground shrink-0 text-xs">
          {relativeTime(chapter.published_at)}
        </time>
      )}
      <ChapterState chapter={chapter} pending={pending} onDownload={onDownload} />
    </li>
  )
}

/**
 * What can be done with this chapter, or what is already happening to it.
 *
 * Downloaded chapters get a status, not a disabled button: a control that
 * cannot be used is noise in a list of forty rows, and "Downloaded" answers
 * the question the reader actually has.
 */
function ChapterState({
  chapter,
  pending,
  onDownload,
}: {
  chapter: ChapterSummary
  pending: boolean
  onDownload: (chapterId: string) => void
}) {
  if (chapter.downloaded) {
    return (
      <span className="text-muted-foreground flex shrink-0 items-center gap-1 text-xs">
        {/* A dot, not a filled row. Colour is never the only carrier — the
            word beside it says the same thing (ADR-0016). */}
        <span className="bg-success size-1.5 rounded-full" aria-hidden="true" />
        <Trans>Downloaded</Trans>
      </span>
    )
  }

  if (pending) {
    return (
      <span className="text-muted-foreground flex shrink-0 items-center gap-1 text-xs">
        <span className="bg-accent size-1.5 rounded-full" aria-hidden="true" />
        <Trans>Queued</Trans>
      </span>
    )
  }

  return (
    <button
      type="button"
      onClick={() => onDownload(chapter.id)}
      className="text-accent hover:bg-accent-soft shrink-0 rounded-sm px-2 py-0.5 text-xs"
    >
      <Trans>Download</Trans>
    </button>
  )
}

/**
 * The chapter number, as the source gave it.
 *
 * A source may omit it, and a decimal one is meaningful — 10.5 is a real
 * chapter, not a rounding error — so trailing zeros are dropped rather than a
 * fixed precision applied.
 */
function formatNumber(number: number | null | undefined): string {
  if (number === null || number === undefined) return '—'
  return String(Number(number.toFixed(2)))
}

/** The chapter's own title, or its number when the source gave none. */
function chapterLabel(chapter: ChapterSummary): string {
  if (chapter.title !== null && chapter.title !== undefined && chapter.title !== '') {
    return chapter.title
  }
  return chapter.external_key
}
