import { queryOptions } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { libraryKeys } from './keys'

/**
 * Query options for the library.
 *
 * `queryOptions` rather than bare hooks so a route loader and a component
 * share one definition: the loader calls `ensureQueryData` with it to prime
 * the cache, and the component calls `useQuery` with the same value. Two
 * definitions would drift, and the one that drifts is the loader — because
 * nothing renders it (ADR-0008).
 */

/**
 * Anything the server returns is already the shape the schema promises, so a
 * failed request is the only thing left to handle. `openapi-fetch` returns
 * `{ data, error }` rather than throwing; Query needs a rejection to mark a
 * query failed, so the error is thrown here.
 *
 * The thrown value is the problem document itself, not a wrapped `Error`, so
 * a component can branch on `type` without unwrapping anything.
 */
function unwrap<D, E>({ data, error }: { data?: D; error?: E }): D {
  if (error !== undefined) throw error
  // `data` is present whenever `error` is not; the assertion records that
  // rather than hiding a real absence.
  return data as D
}

/** Series already in the library, newest first. */
export function libraryListQuery(cursor?: string) {
  return queryOptions({
    queryKey: libraryKeys.list(cursor),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/manga', {
          // Spread rather than `cursor` directly: `exactOptionalPropertyTypes`
          // distinguishes an absent key from one set to `undefined`, and the
          // server's first page is the absence — sending `?cursor=undefined`
          // would be a cursor that fails to parse.
          params: { query: { ...(cursor ? { cursor } : {}) } },
          signal,
        }),
      ),
    // The library changes when a job finishes, and SSE says so. Polling on
    // top of that would be the traffic ADR-0010 chose SSE to avoid.
    staleTime: 30_000,
  })
}

/** One series. */
export function mangaDetailQuery(mangaId: string) {
  return queryOptions({
    queryKey: libraryKeys.detail(mangaId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/manga/{id}', {
          params: { path: { id: mangaId } },
          signal,
        }),
      ),
    staleTime: 30_000,
  })
}

/** A series' chapters. */
export function mangaChaptersQuery(mangaId: string, cursor?: string) {
  return queryOptions({
    queryKey: libraryKeys.chapters(mangaId, cursor),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/manga/{id}/chapters', {
          params: {
            path: { id: mangaId },
            query: { ...(cursor ? { cursor } : {}) },
          },
          signal,
        }),
      ),
    staleTime: 30_000,
  })
}
