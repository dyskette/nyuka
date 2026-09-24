import { useQueries } from '@tanstack/react-query'
import { activeChapterDownloads, type ChapterDownload } from '@/features/jobs/lib/downloads'
import { useProgress } from '@/shared/sse/progress'
import { jobListQuery } from './queries'

/** The job states that still have work coming. */
const ACTIVE_STATES = ['queued', 'running'] as const

/**
 * What the queue is doing to each chapter, by chapter id.
 *
 * Two queries because `GET /jobs` filters on one state at a time. Both are
 * small, both are invalidated by the same events, and asking for every job
 * and filtering here would miss an active one past the first page.
 */
export function useChapterDownloads(): Map<string, ChapterDownload> {
  const results = useQueries({
    queries: ACTIVE_STATES.map((state) => jobListQuery(state)),
  })
  const progress = useProgress()

  // Running first, so a chapter with both a running job and a queued retry
  // reads as running.
  const jobs = results.flatMap((result) => result.data?.items ?? [])

  return activeChapterDownloads(jobs, progress)
}
