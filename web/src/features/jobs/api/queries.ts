import { queryOptions } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { jobKeys } from './keys'

function unwrap<D, E>({ data, error }: { data?: D; error?: E }): D {
  if (error !== undefined) throw error
  return data as D
}

/**
 * The job queue, newest first.
 *
 * `staleTime: 0` unlike the library's 30 s: the queue is the one view whose
 * whole purpose is to be current, and every job event invalidates it anyway.
 * A stale window here would mean an invalidation that does not refetch.
 */
export function jobListQuery(state?: string) {
  return queryOptions({
    queryKey: jobKeys.list(state),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/jobs', {
          params: { query: { ...(state ? { state } : {}) } },
          signal,
        }),
      ),
    staleTime: 0,
  })
}
