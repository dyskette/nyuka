import { describe, expect, it } from 'vitest'
import type { components } from '@/shared/api/schema'
import type { Progress } from '@/shared/sse/progress'
import { activeChapterDownloads } from './downloads'

type JobSummary = components['schemas']['JobSummaryDto']

const CHAPTER = '22222222-2222-4222-8222-222222222222'

function job(overrides: Partial<JobSummary> = {}): JobSummary {
  return {
    id: 'j-1',
    kind: 'download_chapter',
    state: 'running',
    priority: 0,
    run_at: '2026-09-23T12:00:00Z',
    attempts: 0,
    max_attempts: 3,
    created_at: '2026-09-23T12:00:00Z',
    subject: {
      manga_id: '11111111-1111-4111-8111-111111111111',
      manga_title: 'Ashfall Chronicle',
      chapter_id: CHAPTER,
      chapter_number: 80,
      chapter_title: 'The Last Cartographer',
    },
    ...overrides,
  }
}

function progress(overrides: Partial<Progress> = {}): Progress {
  return { done: 12, total: 40, bytes: 1024, bytesPerSecond: 512, ...overrides }
}

describe('activeChapterDownloads', () => {
  it('reports a running job with the pages it has done', () => {
    const map = activeChapterDownloads([job()], new Map([['j-1', progress()]]))

    expect(map.get(CHAPTER)).toEqual({ state: 'running', done: 12, total: 40 })
  })

  /**
   * Progress is not persisted, so a job that is running but has sent nothing
   * yet — or is running after a reload — is known to be running and not known
   * to be anywhere in particular.
   */
  it('reports a running job with no progress yet', () => {
    const map = activeChapterDownloads([job()], new Map())

    expect(map.get(CHAPTER)).toEqual({ state: 'running' })
  })

  it('reports a queued job', () => {
    const map = activeChapterDownloads([job({ state: 'queued' })], new Map())

    expect(map.get(CHAPTER)?.state).toBe('queued')
  })

  /** A finished job is what `downloaded` on the chapter already says. */
  it.each(['succeeded', 'failed', 'cancelled'])('ignores a %s job', (state) => {
    expect(activeChapterDownloads([job({ state })], new Map()).size).toBe(0)
  })

  /** Maintenance work has no chapter to attach to. */
  it('ignores a job with no subject', () => {
    expect(activeChapterDownloads([job({ subject: null })], new Map()).size).toBe(0)
  })

  /**
   * A retried chapter has two jobs. The list is newest first, so the first
   * one seen wins — otherwise the row would show the attempt that already
   * failed rather than the one running now.
   */
  it('keeps the most recent job for a chapter', () => {
    const map = activeChapterDownloads(
      [job({ id: 'new', state: 'running' }), job({ id: 'old', state: 'queued' })],
      new Map(),
    )

    expect(map.get(CHAPTER)?.state).toBe('running')
  })
})
