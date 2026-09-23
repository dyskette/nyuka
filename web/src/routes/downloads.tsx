import { Trans } from '@lingui/react/macro'
import { useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute, Link } from '@tanstack/react-router'
import { z } from 'zod'
import { useCancelJob, useRetryJob } from '@/features/jobs/api/mutations'
import { jobListQuery } from '@/features/jobs/api/queries'
import { JobQueue } from '@/features/jobs/components/JobQueue'
import { useProgress } from '@/shared/sse/progress'

/** The server's job states, which the filter tabs map onto. */
const STATES = ['queued', 'running', 'succeeded', 'failed', 'cancelled'] as const

/**
 * The download queue and everything else the server is doing.
 *
 * "Downloads" in the sidebar, the job queue underneath: a reader thinks in
 * chapters and the server thinks in jobs, and this screen is where the two
 * meet. Maintenance work shows up here too rather than being hidden, because
 * a stuck prune is exactly the kind of thing an operator needs to see.
 */
export const Route = createFileRoute('/downloads')({
  // `state` is a search param so a filtered queue is a link — which is what
  // gets pasted into an issue when something is stuck.
  validateSearch: z.object({ state: z.enum(STATES).optional() }),

  loaderDeps: ({ search }) => ({ state: search.state }),
  loader: ({ context, deps }) => context.queryClient.ensureQueryData(jobListQuery(deps.state)),

  component: DownloadsScreen,
  pendingComponent: QueueSkeleton,
  errorComponent: QueueError,
})

function DownloadsScreen() {
  const { state } = Route.useSearch()
  const { data } = useSuspenseQuery(jobListQuery(state))

  // Live, from the event stream. Not part of the query: the server exposes no
  // endpoint that reports a running job's progress, so there is nothing to
  // refetch it from.
  const progress = useProgress()

  const cancel = useCancelJob()
  const retry = useRetryJob()

  return (
    <div className="flex h-dvh flex-col">
      <StateFilter active={state} />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <JobQueue
          jobs={data.items}
          progress={progress}
          onCancel={(id) => cancel.mutate(id)}
          onRetry={(id) => retry.mutate(id)}
        />
      </div>
    </div>
  )
}

/**
 * The state filter.
 *
 * Links rather than buttons, for the same reason the panel's tabs are: the
 * filter is in the URL, so each one has a real href and the back button
 * undoes a filter change.
 */
function StateFilter({ active }: { active: (typeof STATES)[number] | undefined }) {
  return (
    <nav className="border-border px-cell flex h-row shrink-0 items-center gap-1 border-b text-xs">
      <FilterLink active={active === undefined}>
        <Trans>All</Trans>
      </FilterLink>
      {STATES.map((state) => (
        <FilterLink key={state} state={state} active={active === state}>
          {state}
        </FilterLink>
      ))}
    </nav>
  )
}

function FilterLink({
  state,
  active,
  children,
}: {
  state?: (typeof STATES)[number]
  active: boolean
  children: React.ReactNode
}) {
  return (
    <Link
      to="/downloads"
      // `undefined` clears the key rather than sending `?state=undefined`,
      // which would fail the server's own validation of the filter.
      search={{ ...(state ? { state } : {}) }}
      {...(active ? { 'aria-current': 'page' } : {})}
      className={`rounded-sm px-2 py-0.5 ${
        active ? 'bg-accent-soft text-foreground' : 'text-muted-foreground hover:text-foreground'
      }`}
    >
      {children}
    </Link>
  )
}

/// Stable keys for a fixed-length placeholder list.
const SKELETON_ROWS = Array.from({ length: 10 }, (_, index) => `skeleton-${index}`)

function QueueSkeleton() {
  return (
    <div className="flex flex-col">
      {SKELETON_ROWS.map((id) => (
        <div key={id} className="border-border h-row px-cell flex items-center border-b">
          <div className="bg-surface-raised h-3 w-1/4 animate-pulse rounded-sm" />
        </div>
      ))}
    </div>
  )
}

function QueueError() {
  return (
    <div className="p-8 text-sm">
      <Trans>The job queue could not be loaded.</Trans>
    </div>
  )
}
