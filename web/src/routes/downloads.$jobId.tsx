import { Trans, useLingui } from '@lingui/react/macro'
import { useQuery } from '@tanstack/react-query'
import { createFileRoute, Link } from '@tanstack/react-router'
import { useCancelJob, useRetryJob } from '@/features/jobs/api/mutations'
import { jobQuery } from '@/features/jobs/api/queries'
import { problemMessage } from '@/shared/api/problem'
import type { components } from '@/shared/api/schema'
import { formatBytes, formatRate } from '@/shared/lib/bytes'
import { relativeTime } from '@/shared/lib/time'
import { useProgress } from '@/shared/sse/progress'

type JobSummary = components['schemas']['JobSummaryDto']

/**
 * One job, in detail.
 *
 * A nested route, for the reasons ADR-0017 gives: the queue stays mounted
 * behind it, so its scroll position survives, and the panel gets its own
 * boundaries.
 *
 * # What is not here, and why
 *
 * The mockup shows a per-page grid and a timestamped event log. Neither
 * exists to show: `job.progress` carries `done`, `total` and `bytes` and says
 * nothing about individual pages, and no event history is kept — SSE is a live
 * channel with no replay (ADR-0010), so a panel opened after the fact would
 * have nothing to fill a log with. What is shown is what the server can
 * answer for.
 */
export const Route = createFileRoute('/downloads/$jobId')({
  loader: ({ context, params }) => context.queryClient.ensureQueryData(jobQuery(params.jobId)),
  component: JobPanel,
  errorComponent: PanelError,
})

function JobPanel() {
  const { jobId } = Route.useParams()
  const { data, error, isPending } = useQuery(jobQuery(jobId))
  const progress = useProgress().get(jobId)

  const cancel = useCancelJob()
  const retry = useRetryJob()

  if (isPending) return <PanelSkeleton />
  if (error) {
    return (
      <p className="text-danger p-panel text-sm" role="alert">
        {problemMessage(error) ?? <Trans>This job could not be read.</Trans>}
      </p>
    )
  }

  return (
    <div className="p-panel flex flex-col gap-4">
      <Header job={data} />

      <Facts job={data} progress={progress} />

      {/*
        The last error, in full. The queue row cannot show it — it is one line
        among seven columns — and it is the single most useful thing on this
        panel when something is stuck.
      */}
      {data.last_error !== null && data.last_error !== undefined && (
        <section className="flex flex-col gap-1">
          <h3 className="text-muted-foreground text-xs font-medium">
            <Trans>Last error</Trans>
          </h3>
          <p className="border-danger text-danger rounded-sm border px-3 py-2 font-mono text-xs whitespace-pre-wrap">
            {data.last_error}
          </p>
        </section>
      )}

      <Actions
        job={data}
        onCancel={() => cancel.mutate(data.id)}
        onRetry={() => retry.mutate(data.id)}
      />
    </div>
  )
}

function Header({ job }: { job: JobSummary }) {
  const { t } = useLingui()

  return (
    <header className="flex items-start gap-2">
      <div className="min-w-0 flex-1">
        {job.subject !== null && job.subject !== undefined ? (
          <>
            <h2 className="truncate text-base font-semibold">
              {job.subject.chapter_title ?? job.subject.manga_title}
            </h2>
            <Link
              to="/library/$mangaId"
              params={{ mangaId: job.subject.manga_id }}
              search={{ tab: 'chapters' }}
              className="text-accent truncate text-xs"
            >
              {job.subject.manga_title}
            </Link>
          </>
        ) : (
          // Maintenance work is about nothing a reader named, so its kind is
          // the honest title.
          <h2 className="truncate text-base font-semibold">{job.kind}</h2>
        )}
      </div>

      <Link
        to="/downloads"
        search={(previous) => previous}
        aria-label={t`Close`}
        className="text-muted-foreground hover:text-foreground shrink-0 px-1"
      >
        ×
      </Link>
    </header>
  )
}

/**
 * Everything the server can answer for about this job.
 *
 * Progress comes from the event stream and is absent for a job that has not
 * reported any — which is every queued job, and every running one after a
 * reload. An empty value reads as "not known"; a zero would be a claim.
 */
function Facts({
  job,
  progress,
}: {
  job: JobSummary
  progress: ReturnType<typeof useProgress> extends ReadonlyMap<string, infer V>
    ? V | undefined
    : never
}) {
  return (
    <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-xs">
      <Fact label={<Trans>State</Trans>}>{job.state}</Fact>
      <Fact label={<Trans>Kind</Trans>}>{job.kind}</Fact>
      <Fact label={<Trans>Attempts</Trans>}>
        <span className="tabular">
          {job.attempts}/{job.max_attempts}
        </span>
      </Fact>
      <Fact label={<Trans>Queued</Trans>}>
        <time dateTime={job.created_at}>{relativeTime(job.created_at)}</time>
      </Fact>
      <Fact label={<Trans>Pages</Trans>}>
        {progress ? (
          <span className="tabular">
            {progress.done}/{progress.total}
          </span>
        ) : (
          '—'
        )}
      </Fact>
      <Fact label={<Trans>Downloaded</Trans>}>
        {progress ? <span className="tabular">{formatBytes(progress.bytes)}</span> : '—'}
      </Fact>
      <Fact label={<Trans>Speed</Trans>}>
        {progress?.bytesPerSecond != null ? (
          <span className="tabular">{formatRate(progress.bytesPerSecond)}</span>
        ) : (
          '—'
        )}
      </Fact>
      <Fact label={<Trans>Priority</Trans>}>
        <span className="tabular">{job.priority}</span>
      </Fact>
    </dl>
  )
}

function Fact({ label, children }: { label: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="flex flex-col">
      <dt className="text-muted-foreground">{label}</dt>
      <dd>{children}</dd>
    </div>
  )
}

/** Only what the server will accept, as on the queue row. */
function Actions({
  job,
  onCancel,
  onRetry,
}: {
  job: JobSummary
  onCancel: () => void
  onRetry: () => void
}) {
  if (job.state === 'queued' || job.state === 'running') {
    return (
      <button
        type="button"
        onClick={onCancel}
        className="border-border hover:text-danger rounded-sm border px-3 py-1.5 text-sm"
      >
        <Trans>Cancel this job</Trans>
      </button>
    )
  }

  if (job.state === 'failed') {
    return (
      <button
        type="button"
        onClick={onRetry}
        className="border-border hover:bg-accent-soft text-accent rounded-sm border px-3 py-1.5 text-sm"
      >
        <Trans>Retry this job</Trans>
      </button>
    )
  }

  return null
}

/// Stable keys for a fixed-length placeholder list.
const SKELETON_ROWS = Array.from({ length: 5 }, (_, index) => `skeleton-${index}`)

function PanelSkeleton() {
  return (
    <div className="p-panel flex flex-col gap-2">
      {SKELETON_ROWS.map((id) => (
        <div key={id} className="bg-surface-raised h-3 w-full animate-pulse rounded-sm" />
      ))}
    </div>
  )
}

function PanelError() {
  return (
    <div className="p-panel text-sm">
      <Trans>This job could not be loaded.</Trans>
    </div>
  )
}
