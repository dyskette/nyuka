import { queryOptions } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { followKeys } from './keys'

function unwrap<D, E>({ data, error }: { data?: D; error?: E }): D {
  if (error !== undefined) throw error
  return data as D
}

/** Followed series, newest first, with what each one is waiting on. */
export function followListQuery(cursor?: string) {
  return queryOptions({
    queryKey: followKeys.list(cursor),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/follows', {
          params: { query: { ...(cursor ? { cursor } : {}) } },
          signal,
        }),
      ),
    staleTime: 30_000,
  })
}
