/**
 * Query keys for follows (ADR-0009).
 *
 * Its own root rather than under the library: a follow outlives any one view
 * of the series it watches, and a chapter landing invalidates the library
 * without needing to throw away this list.
 */
export const followKeys = {
  all: ['follows'] as const,

  lists: () => [...followKeys.all, 'list'] as const,
  list: (cursor?: string) => [...followKeys.lists(), { cursor: cursor ?? null }] as const,

  details: () => [...followKeys.all, 'detail'] as const,
  detail: (followId: string) => [...followKeys.details(), followId] as const,
}
