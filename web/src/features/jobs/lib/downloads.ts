import type { components } from '@/shared/api/schema'
import type { Progress } from '@/shared/sse/progress'

type JobSummary = components['schemas']['JobSummaryDto']

/** What is happening to a chapter right now, if anything. */
export interface ChapterDownload {
  state: 'queued' | 'running'
  /** Pages finished and expected, once the first progress event has arrived. */
  done?: number
  total?: number
}

/** The job states that mean work is still coming. */
const ACTIVE = new Set(['queued', 'running'])

/**
 * Which chapters have a download under way, by chapter id.
 *
 * A chapter's own row carries no such state: `ChapterSummaryDto` says whether
 * a file exists, and between asking for one and it landing there is nothing on
 * the chapter to read. The answer lives in the queue, so this is where the two
 * are joined.
 *
 * Page counts come from the live event stream rather than the job row, since
 * progress is not persisted — a running job whose first event has not arrived
 * is known to be running and not yet known to be anywhere in particular.
 */
export function activeChapterDownloads(
  jobs: JobSummary[],
  progress: ReadonlyMap<string, Progress>,
): Map<string, ChapterDownload> {
  const out = new Map<string, ChapterDownload>()

  for (const job of jobs) {
    if (!ACTIVE.has(job.state)) continue
    const chapterId = job.subject?.chapter_id
    if (chapterId === null || chapterId === undefined) continue

    const state = job.state === 'running' ? 'running' : 'queued'
    const ticks = progress.get(job.id)

    // Newest first from the server, so an earlier entry is the more recent
    // job for this chapter and a retry does not read as the failed attempt.
    if (out.has(chapterId)) continue

    out.set(
      chapterId,
      ticks === undefined ? { state } : { state, done: ticks.done, total: ticks.total },
    )
  }

  return out
}
