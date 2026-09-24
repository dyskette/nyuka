import { Trans, useLingui } from '@lingui/react/macro'
import type { ChapterDownload } from '@/features/jobs/lib/downloads'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'

type ChapterSummary = components['schemas']['ChapterSummaryDto']

export interface ChapterListProps {
  chapters: ChapterSummary[]
  /**
   * What is happening to each chapter right now, by chapter id.
   *
   * Not on the chapter itself: `ChapterSummaryDto` says whether a file
   * exists, and between asking for one and it landing there is nothing on the
   * chapter to read. It comes from the queue.
   */
  downloading?: ReadonlyMap<string, ChapterDownload>
  onDownload: (chapterId: string) => void
}

/** A series' chapters, with what the reader can do about each. */
export function ChapterList({ chapters, downloading, onDownload }: ChapterListProps) {
  if (chapters.length === 0) {
    return (
      // A series' chapter list is read when it is added. Refreshing one
      // afterwards is a capability the API does not have — see the gap table
      // in `docs/design/README.md` — so this says what happened rather than
      // naming an action nobody can take.
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>No chapters. The source listed none when this series was added.</Trans>
      </p>
    )
  }

  return (
    <ul className="flex flex-col">
      {/* The server orders them newest first, so the page order is the
          reading order and paging on keeps it. */}
      {chapters.map((chapter) => (
        <ChapterRow
          key={chapter.id}
          chapter={chapter}
          download={downloading?.get(chapter.id)}
          onDownload={onDownload}
        />
      ))}
    </ul>
  )
}

function ChapterRow({
  chapter,
  download,
  onDownload,
}: {
  chapter: ChapterSummary
  download: ChapterDownload | undefined
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
      <ChapterState chapter={chapter} download={download} onDownload={onDownload} />
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
  download,
  onDownload,
}: {
  chapter: ChapterSummary
  download: ChapterDownload | undefined
  onDownload: (chapterId: string) => void
}) {
  const { t } = useLingui()

  if (chapter.downloaded) {
    return (
      <span className="text-muted-foreground flex shrink-0 items-center gap-2 text-xs">
        {/* A dot, not a filled row. Colour is never the only carrier — the
            word beside it says the same thing (ADR-0016). */}
        <span className="flex items-center gap-1">
          <span className="bg-success size-1.5 rounded-full" aria-hidden="true" />
          <Trans>Downloaded</Trans>
        </span>

        {/*
          A plain link, not a fetch. The endpoint is same-origin and answers a
          `GET` with the session cookie, so the browser's own download handles
          it — ranged, resumable, and streamed rather than held in memory the
          way a blob would be.

          No `download` attribute: the server sends `Content-Disposition` with
          the archive's real name, which already carries the series, volume and
          chapter in the shape other readers parse (ADR-0007). Setting one here
          would override that with whatever this page happened to know.
        */}
        <a
          href={`/api/v1/downloads/${chapter.id}/file`}
          aria-label={t`Save ${chapterLabel(chapter)} to this device`}
          className="text-accent hover:bg-accent-soft rounded-sm px-1.5 py-0.5"
        >
          <Trans>Save</Trans>
        </a>
      </span>
    )
  }

  if (download !== undefined) {
    return (
      <span className="text-muted-foreground flex shrink-0 items-center gap-1.5 text-xs">
        {/* Pulsing while it runs, still while it waits: the difference
            between the two is the thing a reader is looking for, and it is
            carried by the word beside it as well (ADR-0016). */}
        <span
          className={`bg-accent size-1.5 rounded-full ${
            download.state === 'running' ? 'animate-pulse' : ''
          }`}
          aria-hidden="true"
        />
        {download.state === 'queued' ? (
          <Trans>Queued</Trans>
        ) : download.total !== undefined && download.done !== undefined ? (
          <span className="tabular">
            <Trans>
              Downloading {download.done}/{download.total}
            </Trans>
          </span>
        ) : (
          <Trans>Downloading</Trans>
        )}
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
  // The key is only worth showing when it says something the number column
  // does not. Sources commonly key a chapter by its number, and printing it
  // again beside itself reads as a rendering fault.
  const number = chapter.number != null ? String(Number(chapter.number)) : null
  return chapter.external_key === number ? '' : chapter.external_key
}
