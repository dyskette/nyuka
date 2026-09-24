import { Trans, useLingui } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import type { components } from '@/shared/api/schema'
import { formatBytes, formatRate } from '@/shared/lib/bytes'
import { openRowLink } from '@/shared/lib/rowClick'
import { relativeTime } from '@/shared/lib/time'
import type { Progress } from '@/shared/sse/progress'

type JobSummary = components['schemas']['JobSummaryDto']

export interface JobQueueProps {
  jobs: JobSummary[]
  /** Live progress by job id, from the event stream. */
  progress: ReadonlyMap<string, Progress>
  onCancel: (jobId: string) => void
  onRetry: (jobId: string) => void
}

/**
 * The work queue, as an operator watches it.
 *
 * A table for the same reason the library is one: every column is a number or
 * a state, and the question being asked is "what is happening and what is
 * stuck".
 */
export function JobQueue({ jobs, progress, onCancel, onRetry }: JobQueueProps) {
  if (jobs.length === 0) return <EmptyQueue />

  return (
    <table className="w-full border-collapse text-sm">
      <caption className="sr-only">
        <Trans>Background jobs</Trans>
      </caption>
      <thead>
        <tr className="border-border text-muted-foreground px-cell border-b text-left text-xs">
          <Th>
            <Trans>Job</Trans>
          </Th>
          <Th>
            <Trans>State</Trans>
          </Th>
          <Th>
            <Trans>Progress</Trans>
          </Th>
          <Th align="right">
            <Trans>Size</Trans>
          </Th>
          <Th align="right">
            <Trans>Speed</Trans>
          </Th>
          <Th>
            <Trans>Queued</Trans>
          </Th>
          <th scope="col" className="px-cell h-row w-0">
            <span className="sr-only">
              <Trans>Actions</Trans>
            </span>
          </th>
        </tr>
      </thead>
      <tbody>
        {jobs.map((job) => (
          <Row
            key={job.id}
            job={job}
            progress={progress.get(job.id)}
            onCancel={onCancel}
            onRetry={onRetry}
          />
        ))}
      </tbody>
    </table>
  )
}

function Th({ children, align = 'left' }: { children: React.ReactNode; align?: 'left' | 'right' }) {
  return (
    <th
      scope="col"
      className={`px-cell h-row font-medium ${align === 'right' ? 'text-right' : ''}`}
    >
      {children}
    </th>
  )
}

function Row({
  job,
  progress,
  onCancel,
  onRetry,
}: {
  job: JobSummary
  progress: Progress | undefined
  onCancel: (jobId: string) => void
  onRetry: (jobId: string) => void
}) {
  return (
    <tr
      onClick={openRowLink}
      className="border-border hover:bg-surface-raised cursor-pointer border-b"
    >
      <td className="px-cell h-row w-full max-w-0">
        <JobLabel job={job} />
      </td>
      <td className="px-cell h-row">
        <JobState job={job} />
      </td>
      <td className="px-cell h-row">
        <ProgressCell progress={progress} />
      </td>
      <td className="px-cell tabular text-muted-foreground h-row text-right text-xs">
        {progress ? formatBytes(progress.bytes) : '—'}
      </td>
      <td className="px-cell tabular text-muted-foreground h-row text-right text-xs">
        {progress?.bytesPerSecond != null ? formatRate(progress.bytesPerSecond) : '—'}
      </td>
      <td className="px-cell text-muted-foreground h-row text-xs">
        <time dateTime={job.created_at}>{relativeTime(job.created_at)}</time>
      </td>
      <td className="px-cell h-row">
        <Actions job={job} onCancel={onCancel} onRetry={onRetry} />
      </td>
    </tr>
  )
}

/**
 * What the job is about.
 *
 * A download names its chapter and links to the series. Maintenance work has
 * no subject, so it shows its kind — which is the honest answer, rather than
 * a uuid that answers nothing.
 */
function JobLabel({ job }: { job: JobSummary }) {
  const { subject } = job

  // The label opens the job, not the series. A row in a queue is about the
  // work, and the panel links on to the series for anyone who wanted that
  // instead — the reverse would leave no way to reach the job at all.
  if (subject === null || subject === undefined) {
    return (
      <Link
        to="/downloads/$jobId"
        params={{ jobId: job.id }}
        search={(previous) => previous}
        data-row-link=""
        className="text-muted-foreground block truncate"
      >
        {job.kind}
      </Link>
    )
  }

  const number = subject.chapter_number != null ? String(Number(subject.chapter_number)) : null

  return (
    <Link
      to="/downloads/$jobId"
      params={{ jobId: job.id }}
      search={(previous) => previous}
      data-row-link=""
      className="flex min-w-0 items-baseline gap-1.5"
    >
      {number !== null && (
        <span className="tabular text-muted-foreground shrink-0 text-xs">{number}</span>
      )}
      <span className="truncate">{subject.chapter_title ?? subject.manga_title}</span>
      {/* The series is the secondary half of this label, so it gives way
          first. `shrink-0` would leave `truncate` nothing to act on. */}
      {subject.chapter_title !== null && subject.chapter_title !== undefined && (
        <span className="text-muted-foreground min-w-0 max-w-40 shrink truncate text-xs">
          {subject.manga_title}
        </span>
      )}
    </Link>
  )
}

/**
 * The job's state, as a dot and a word.
 *
 * A failed job that still has attempts is shown with its attempt count, since
 * "failed 2/3" and "failed 3/3" are different situations: one is going to run
 * again and the other is not.
 */
function JobState({ job }: { job: JobSummary }) {
  const dot =
    {
      running: 'bg-accent',
      queued: 'bg-muted-foreground',
      succeeded: 'bg-success',
      failed: 'bg-danger',
      cancelled: 'bg-muted-foreground',
    }[job.state] ?? 'bg-muted-foreground'

  return (
    <span className="flex items-center gap-1.5 text-xs">
      <span className={`${dot} size-1.5 shrink-0 rounded-full`} aria-hidden="true" />
      <span>{job.state}</span>
      {job.state === 'failed' && job.attempts < job.max_attempts && (
        <span className="text-muted-foreground tabular">
          {job.attempts}/{job.max_attempts}
        </span>
      )}
    </span>
  )
}

/**
 * The live progress bar.
 *
 * Absent when no event has arrived — which is the case for a queued job, and
 * for a running one after a reload, since progress is not persisted anywhere.
 * An empty cell says "not known"; a zero-width bar would say "no progress",
 * which is a different claim.
 */
function ProgressCell({ progress }: { progress: Progress | undefined }) {
  const { t } = useLingui()

  if (progress === undefined) return <span className="text-muted-foreground text-xs">—</span>

  const fraction = progress.total > 0 ? progress.done / progress.total : 0

  return (
    <span className="flex items-center gap-2">
      <span
        className="bg-surface-raised h-1 w-24 shrink-0 overflow-hidden rounded-sm"
        aria-hidden="true"
      >
        <span
          className="bg-accent block h-full rounded-sm"
          style={{
            width: `${fraction * 100}%`,
            transition: 'width var(--dur-slow) var(--ease-out)',
          }}
        />
      </span>
      {/* Two spans: a `span` has the generic role, which takes no
          accessible name, so an `aria-label` here is dropped. */}
      <span className="tabular text-muted-foreground text-xs" aria-hidden="true">
        {progress.done}/{progress.total}
      </span>
      <span className="sr-only">{t`${progress.done} of ${progress.total} pages`}</span>
    </span>
  )
}

function Actions({
  job,
  onCancel,
  onRetry,
}: {
  job: JobSummary
  onCancel: (jobId: string) => void
  onRetry: (jobId: string) => void
}) {
  const { t } = useLingui()

  // Only what the server will accept. Offering "retry" on a running job would
  // be a button whose only outcome is an error.
  if (job.state === 'queued' || job.state === 'running') {
    return (
      <button
        type="button"
        onClick={() => onCancel(job.id)}
        aria-label={t`Cancel this job`}
        className="text-muted-foreground hover:text-danger rounded-sm px-2 py-0.5 text-xs"
      >
        <Trans>Cancel</Trans>
      </button>
    )
  }

  if (job.state === 'failed') {
    return (
      <button
        type="button"
        onClick={() => onRetry(job.id)}
        aria-label={t`Retry this job`}
        className="text-accent hover:bg-accent-soft rounded-sm px-2 py-0.5 text-xs"
      >
        <Trans>Retry</Trans>
      </button>
    )
  }

  return null
}

function EmptyQueue() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>Nothing in the queue</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>Downloads and scheduled work appear here while they run.</Trans>
      </p>
    </div>
  )
}
