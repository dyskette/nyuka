import type { QueryClient } from '@tanstack/react-query'
import { jobKeys } from '@/features/jobs/api/keys'
import { libraryKeys } from '@/features/library/api/keys'
import type { EventName, EventPayloads, JobProgress, JobStateChanged } from './events'
import { clearProgress, recordProgress } from './progress'

/**
 * Job states a job does not come back from.
 *
 * The server sends these capitalised, as Rust names them — `"Succeeded"`, not
 * `"succeeded"`. A lowercase comparison here would match nothing and the
 * progress bars would never be cleared.
 */
const TERMINAL = new Set(['Succeeded', 'Failed', 'Cancelled'])

/**
 * What each event makes stale.
 *
 * Handlers touch only the cache — they never hold their own copy of anything
 * (ADR-0010). Invalidation rather than a direct `setQueryData` write, because
 * an event carries ids and not the rows a list is paged into: reconstructing
 * a keyset page from an event is where a live UI starts disagreeing with the
 * server.
 *
 * Each returns the keys to invalidate, so this table is a pure function and
 * testable without a `QueryClient`.
 */
export const INVALIDATES: {
  [N in EventName]: (payload: EventPayloads[N]) => readonly (readonly unknown[])[]
} = {
  // A new chapter changes the series' chapter list and its library row's
  // count, and nothing else.
  'chapter.new': (p) => [libraryKeys.detail(p.manga_id), libraryKeys.lists()],

  // Same, plus the downloaded count — which is why the event carries
  // `manga_id`. Without it the only correct reaction would be to refetch the
  // whole library.
  'chapter.downloaded': (p) => [libraryKeys.detail(p.manga_id), libraryKeys.lists()],

  // A progress tick invalidates nothing: it is emitted once per downloaded
  // page, and refetching on each is the traffic ADR-0010 chose SSE to avoid.
  // The numbers go to the progress store instead — see `applyEvent`.
  'job.progress': () => [],

  // A finished job may have changed anything the job did, and the event does
  // not say what. The list is refetched; the series is not, because a job
  // that touched one already emitted a chapter event for it.
  'job.state': () => [libraryKeys.lists(), jobKeys.all],

  // A source's metadata changed, which is what the library list renders as a
  // source name.
  'source.updated': () => [libraryKeys.lists(), jobKeys.all],
}

/**
 * Applies an event to the cache.
 *
 * Unknown names are ignored rather than thrown on: a server emitting an event
 * this build does not know about is a version skew, and a thrown error inside
 * an `EventSource` listener would kill the stream for every other event too.
 */
export function applyEvent<N extends EventName>(
  queryClient: QueryClient,
  name: N,
  payload: EventPayloads[N],
): void {
  // Progress is not cache state — it has no endpoint to refetch from — so it
  // goes to its own store rather than through an invalidation.
  if (name === 'job.progress') {
    recordProgress(payload as JobProgress)
    return
  }

  // A job that reached a terminal state has no progress left to show, and its
  // last bar would otherwise stay on screen for the life of the tab.
  if (name === 'job.state' && TERMINAL.has((payload as JobStateChanged).state)) {
    clearProgress((payload as JobStateChanged).job_id)
  }

  const keys = INVALIDATES[name]?.(payload) ?? []
  for (const queryKey of keys) {
    void queryClient.invalidateQueries({ queryKey })
  }
}

/**
 * Everything that must be refetched after a reconnect.
 *
 * This is where correctness comes from, not from replay (ADR-0010): the
 * client cannot know what it missed while disconnected, so it assumes it
 * missed everything. The same applies to a `lagged` event, which says exactly
 * that.
 */
export function invalidateEverything(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: libraryKeys.all })
  void queryClient.invalidateQueries({ queryKey: jobKeys.all })
}
