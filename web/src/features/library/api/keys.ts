/** Everything that changes what `GET /manga` returns. */
export interface LibraryListParams {
  q?: string | undefined
  status?: string | undefined
  source_id?: string | undefined
  sort?: string | undefined
  dir?: string | undefined
  cursor?: string | undefined
}

/**
 * Query keys for the library feature.
 *
 * Resource-first and hierarchical, so `invalidateQueries({ queryKey:
 * libraryKeys.all })` reaches every cached library query through prefix
 * matching — including ones produced by different endpoints (ADR-0009).
 *
 * This is the public interface of the feature's data layer. SSE handlers,
 * route loaders, and components address the cache only through it.
 */
export const libraryKeys = {
  all: ['library'] as const,

  lists: () => [...libraryKeys.all, 'list'] as const,
  /**
   * Keyed on everything that changes what the server returns.
   *
   * One object rather than a segment per parameter: appending segments would
   * mean a new key shape each time one is added, and every stored entry would
   * stop matching at once — which reads as a cache that lost everything for
   * no reason.
   *
   * The cursor is part of the key because each page is its own entry, and the
   * ordering is part of it because a cursor only means anything under the
   * order it was taken in.
   */
  list: (params: LibraryListParams = {}) =>
    [
      ...libraryKeys.lists(),
      {
        q: params.q ?? null,
        status: params.status ?? null,
        source_id: params.source_id ?? null,
        sort: params.sort ?? null,
        dir: params.dir ?? null,
        cursor: params.cursor ?? null,
      },
    ] as const,

  details: () => [...libraryKeys.all, 'detail'] as const,
  detail: (mangaId: string) => [...libraryKeys.details(), mangaId] as const,

  chapters: (mangaId: string, cursor?: string) =>
    [...libraryKeys.detail(mangaId), 'chapters', { cursor: cursor ?? null }] as const,
}
