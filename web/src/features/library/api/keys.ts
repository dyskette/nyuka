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
   * Keyed on the cursor because each page is its own cache entry.
   *
   * The object rather than a bare string keeps the shape stable when another
   * parameter is added: appending a segment would invalidate every stored key
   * at once, which reads as a cache that lost everything for no reason.
   */
  list: (cursor?: string) => [...libraryKeys.lists(), { cursor: cursor ?? null }] as const,

  details: () => [...libraryKeys.all, 'detail'] as const,
  detail: (mangaId: string) => [...libraryKeys.details(), mangaId] as const,

  chapters: (mangaId: string, cursor?: string) =>
    [...libraryKeys.detail(mangaId), 'chapters', { cursor: cursor ?? null }] as const,
}
