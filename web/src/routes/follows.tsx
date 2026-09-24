import { Trans } from '@lingui/react/macro'
import { useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import { z } from 'zod'
import { useCheckFollowNow, useUnfollow, useUpsertFollow } from '@/features/follows/api/mutations'
import { followListQuery } from '@/features/follows/api/queries'
import { FollowTable } from '@/features/follows/components/FollowTable'
import { problemMessage } from '@/shared/api/problem'

/**
 * Followed series and their schedules.
 *
 * The one number a reader acts on is what each follow is waiting to download.
 * Everything else on this screen is the schedule that produces it.
 */
export const Route = createFileRoute('/follows')({
  validateSearch: z.object({ cursor: z.string().optional() }),

  loaderDeps: ({ search }) => ({ cursor: search.cursor }),
  loader: ({ context, deps }) => context.queryClient.ensureQueryData(followListQuery(deps.cursor)),

  component: FollowsScreen,
  pendingComponent: FollowsSkeleton,
  errorComponent: FollowsError,
})

function FollowsScreen() {
  const { cursor } = Route.useSearch()
  const { data } = useSuspenseQuery(followListQuery(cursor))

  const checkNow = useCheckFollowNow()
  const unfollow = useUnfollow()
  const upsert = useUpsertFollow()

  // A row is busy while anything about it is in flight, so its controls do not
  // accept a second change before the first has landed.
  const busy = new Set(
    [
      checkNow.isPending ? checkNow.variables : undefined,
      unfollow.isPending ? unfollow.variables : undefined,
    ].filter((id): id is string => id !== undefined),
  )

  return (
    <div className="flex h-full flex-col">
      <header className="border-border px-cell flex h-row shrink-0 items-center border-b">
        <h1 className="text-sm font-medium">
          <Trans>Follows</Trans>
        </h1>
        <span className="text-muted-foreground tabular ml-auto text-xs">
          <Trans>{data.items.length} followed</Trans>
        </span>
      </header>

      <MutationError error={checkNow.error ?? unfollow.error ?? upsert.error} />

      <div className="min-h-0 flex-1 overflow-y-auto">
        <FollowTable
          follows={data.items}
          busy={busy}
          onCheckNow={(id) => checkNow.mutate(id)}
          onUnfollow={(id) => unfollow.mutate(id)}
          // Both edits go through the same upsert, keyed on the series: a
          // follow is identified by what it watches, so changing one field
          // does not need the other's current value sent with it.
          onIntervalChange={(mangaId, seconds) =>
            upsert.mutate({ manga_id: mangaId, check_interval_secs: seconds })
          }
          onAutoDownloadChange={(mangaId, enabled) =>
            upsert.mutate({ manga_id: mangaId, auto_download: enabled })
          }
        />
      </div>
    </div>
  )
}

function MutationError({ error }: { error: unknown }) {
  if (error === null || error === undefined) return null

  return (
    <p role="alert" className="border-danger text-danger m-3 rounded-sm border px-3 py-2 text-sm">
      {problemMessage(error) ?? <Trans>That did not work.</Trans>}
    </p>
  )
}

/// Stable keys for a fixed-length placeholder list.
const SKELETON_ROWS = Array.from({ length: 8 }, (_, index) => `skeleton-${index}`)

function FollowsSkeleton() {
  return (
    <div className="flex flex-col">
      {SKELETON_ROWS.map((id) => (
        <div key={id} className="border-border h-row px-cell flex items-center border-b">
          <div className="bg-surface-raised h-3 w-1/3 animate-pulse rounded-sm" />
        </div>
      ))}
    </div>
  )
}

function FollowsError() {
  return (
    <div className="p-8 text-sm">
      <Trans>Follows could not be loaded.</Trans>
    </div>
  )
}
