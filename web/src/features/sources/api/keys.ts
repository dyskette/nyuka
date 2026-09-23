/**
 * Query keys for sources, their repositories, and their catalogs (ADR-0009).
 *
 * The catalog hangs off the source rather than living at the root, so
 * uninstalling a source invalidates everything browsed from it with one
 * prefix.
 */
export const sourceKeys = {
  all: ['sources'] as const,

  lists: () => [...sourceKeys.all, 'list'] as const,
  list: () => [...sourceKeys.lists()] as const,

  details: () => [...sourceKeys.all, 'detail'] as const,
  detail: (sourceId: string) => [...sourceKeys.details(), sourceId] as const,

  filters: (sourceId: string) => [...sourceKeys.detail(sourceId), 'filters'] as const,
  settings: (sourceId: string) => [...sourceKeys.detail(sourceId), 'settings'] as const,

  catalogs: (sourceId: string) => [...sourceKeys.detail(sourceId), 'catalog'] as const,
  /**
   * Keyed on the query and cursor: a search is a different listing, not a
   * filtered view of one, because the source decides what matches.
   */
  catalog: (sourceId: string, q?: string, cursor?: string) =>
    [...sourceKeys.catalogs(sourceId), { q: q ?? null, cursor: cursor ?? null }] as const,
}

/** Repositories are a separate resource: a repo outlives the sources from it. */
export const repoKeys = {
  all: ['source-repos'] as const,

  lists: () => [...repoKeys.all, 'list'] as const,
  list: () => [...repoKeys.lists()] as const,

  details: () => [...repoKeys.all, 'detail'] as const,
  detail: (repoId: string) => [...repoKeys.details(), repoId] as const,

  /** What a repository offers, installed or not. */
  available: (repoId: string) => [...repoKeys.detail(repoId), 'available'] as const,
}
