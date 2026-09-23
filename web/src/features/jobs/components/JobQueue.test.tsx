import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import type { Progress } from '@/shared/sse/progress'
import { renderWithProviders } from '@/test/render'
import { JobQueue, type JobQueueProps } from './JobQueue'

type JobSummary = components['schemas']['JobSummaryDto']

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
      chapter_id: '22222222-2222-4222-8222-222222222222',
      chapter_number: 80,
      chapter_title: 'The Last Cartographer',
    },
    ...overrides,
  }
}

function progress(overrides: Partial<Progress> = {}): Progress {
  return { done: 18, total: 42, bytes: 34 * 1024 * 1024, bytesPerSecond: 1024 * 1024, ...overrides }
}

function props(overrides: Partial<JobQueueProps> = {}): JobQueueProps {
  return {
    jobs: [job()],
    progress: new Map(),
    onCancel: vi.fn(),
    onRetry: vi.fn(),
    ...overrides,
  }
}

describe('JobQueue', () => {
  /**
   * The whole reason `JobSummaryDto` resolves a subject server-side: without
   * it this row is a uuid, which answers nothing anyone asks of a queue.
   */
  it('names the chapter a download is for', async () => {
    await renderWithProviders(<JobQueue {...props()} />)

    const row = screen.getByRole('row', { name: /The Last Cartographer/ })
    expect(within(row).getByText('80')).toBeInTheDocument()
    expect(within(row).getByText('Ashfall Chronicle')).toBeInTheDocument()
    // The row opens the *job*. A queue row is about the work; the panel links
    // on to the series for anyone who wanted that instead, and the reverse
    // would leave no way to reach the job at all.
    expect(within(row).getByRole('link').getAttribute('href')).toContain('/downloads/j-1')
  })

  /** Maintenance work is about nothing a reader named, so it shows its kind. */
  it('falls back to the kind when a job has no subject', async () => {
    await renderWithProviders(
      <JobQueue {...props({ jobs: [job({ kind: 'prune_sessions', subject: null })] })} />,
    )

    // Still a link — a maintenance job has a panel too, it just has no series
    // to name in it.
    const link = screen.getByRole('link', { name: 'prune_sessions' })
    expect(link.getAttribute('href')).toContain('/downloads/j-1')
  })

  describe('progress', () => {
    it('shows the live numbers for a job that reported them', async () => {
      await renderWithProviders(
        <JobQueue {...props({ progress: new Map([['j-1', progress()]]) })} />,
      )

      expect(screen.getByText('18/42')).toBeInTheDocument()
      expect(screen.getByText('34 MiB')).toBeInTheDocument()
      expect(screen.getByText('1.0 MiB/s')).toBeInTheDocument()
    })

    /**
     * Progress is not persisted, so a running job has none after a reload. An
     * empty cell says "not known"; a zero-width bar would say "no progress",
     * which is a different and wrong claim.
     */
    it('shows nothing rather than zero when no event has arrived', async () => {
      await renderWithProviders(<JobQueue {...props()} />)

      const row = screen.getByRole('row', { name: /The Last Cartographer/ })
      expect(within(row).queryByText('0/0')).not.toBeInTheDocument()
      expect(within(row).getAllByText('—').length).toBeGreaterThan(0)
    })

    it('omits a rate it could not derive', async () => {
      await renderWithProviders(
        <JobQueue
          {...props({ progress: new Map([['j-1', progress({ bytesPerSecond: null })]]) })}
        />,
      )

      expect(screen.getByText('18/42')).toBeInTheDocument()
      expect(screen.queryByText(/\/s$/)).not.toBeInTheDocument()
    })
  })

  describe('actions', () => {
    /**
     * Only what the server will accept. A retry button on a running job is a
     * control whose only outcome is an error.
     */
    it('offers cancel while a job can still be cancelled', async () => {
      const onCancel = vi.fn()
      await renderWithProviders(<JobQueue {...props({ onCancel })} />)

      await userEvent.click(screen.getByRole('button', { name: 'Cancel this job' }))
      expect(onCancel).toHaveBeenCalledWith('j-1')
      expect(screen.queryByRole('button', { name: 'Retry this job' })).not.toBeInTheDocument()
    })

    it('offers retry only on a failed job', async () => {
      const onRetry = vi.fn()
      await renderWithProviders(
        <JobQueue {...props({ jobs: [job({ state: 'failed', attempts: 3 })], onRetry })} />,
      )

      await userEvent.click(screen.getByRole('button', { name: 'Retry this job' }))
      expect(onRetry).toHaveBeenCalledWith('j-1')
      expect(screen.queryByRole('button', { name: 'Cancel this job' })).not.toBeInTheDocument()
    })

    it('offers nothing on a job that already succeeded', async () => {
      await renderWithProviders(<JobQueue {...props({ jobs: [job({ state: 'succeeded' })] })} />)

      expect(screen.queryAllByRole('button')).toHaveLength(0)
    })
  })

  /**
   * "failed 2/3" and "failed 3/3" are different situations — one runs again
   * and one does not — and the row is where that difference is visible.
   */
  it('shows the attempt count on a failure that will retry', async () => {
    await renderWithProviders(
      <JobQueue {...props({ jobs: [job({ state: 'failed', attempts: 2, max_attempts: 3 })] })} />,
    )

    expect(screen.getByText('2/3')).toBeInTheDocument()
  })

  it('does not show an attempt count on a final failure', async () => {
    await renderWithProviders(
      <JobQueue {...props({ jobs: [job({ state: 'failed', attempts: 3, max_attempts: 3 })] })} />,
    )

    expect(screen.queryByText('3/3')).not.toBeInTheDocument()
  })

  it('explains an empty queue instead of rendering an empty table', async () => {
    await renderWithProviders(<JobQueue {...props({ jobs: [] })} />)

    expect(screen.queryByRole('table')).not.toBeInTheDocument()
    expect(screen.getByText(/Nothing in the queue/)).toBeInTheDocument()
  })
})
