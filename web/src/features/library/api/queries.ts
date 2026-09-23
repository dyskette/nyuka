import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { type LibraryListParams, libraryKeys } from './keys'

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

/**
 * A page of the library, as the server ordered and filtered it.
 *
 * Every parameter goes to the server. Ordering or filtering here would only
 * ever apply to the page in hand, which describes what is on screen rather
 * than what is in the library — a difference that is invisible until the
 * library outgrows one page, and then every count is wrong.
 */
export function libraryListQuery(params: LibraryListParams = {}) {
  return queryOptions({
    queryKey: libraryKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/manga', {
          // Spread rather than passing the object: `exactOptionalPropertyTypes`
          // distinguishes an absent key from one set to `undefined`, and the
          // server's default is the absence — sending `?cursor=undefined`
          // would be a cursor that fails to parse.
          params: { query: defined(params) },
          signal,
        }),
      ),
    // The library changes when a job finishes, and SSE says so. Polling on
    // top of that would be the traffic ADR-0010 chose SSE to avoid.
    staleTime: 30_000,
  })
}

/** Drops the keys that are absent, so none is sent as the string "undefined". */
function defined(params: LibraryListParams): Record<string, string> {
  return Object.fromEntries(
    Object.entries(params).filter((entry): entry is [string, string] => entry[1] !== undefined),
  )
}

/**
 * The library as one growing list.
 *
 * The cursor is the page parameter, so it is deliberately *not* part of the
 * query key here: every page belongs to one cache entry, and that entry is
 * what survives opening the detail panel. Keying on the cursor as
 * `libraryListQuery` does would make each page its own entry and scrolling
 * back would refetch.
 *
 * `initialPageParam` is `undefined` — the server's first page is the absence
 * of a cursor, not a cursor with a special value.
 */
export function libraryInfiniteQuery(params: Omit<LibraryListParams, 'cursor'> = {}) {
  return infiniteQueryOptions({
    queryKey: libraryKeys.list(params),
    queryFn: async ({ pageParam, signal }) =>
      unwrap(
        await api.GET('/manga', {
          params: {
            query: {
              ...defined(params),
              ...(pageParam ? { cursor: pageParam } : {}),
            },
          },
          signal,
        }),
      ),
    initialPageParam: undefined as string | undefined,
    // `next_cursor` absent is the end. Returning `undefined` is what tells
    // Query there is no next page; returning `null` would be a page parameter
    // of null and one more request that returns nothing.
    getNextPageParam: (last) => last.next_cursor ?? undefined,
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

/** A series' chapters, each with its download state. */
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
