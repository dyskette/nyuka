/**
 * Query keys for the jobs feature.
 *
 * Resource-first and hierarchical, so `invalidateQueries({ queryKey:
 * jobsKeys.all })` reaches every cached jobs query through prefix matching —
 * including ones produced by different endpoints (ADR-0009).
 *
 * This is the public interface of the feature's data layer. SSE handlers,
 * route loaders, and components address the cache only through it.
 */
export const jobsKeys = {
  all: ['jobs'] as const,
  lists: () => [...jobsKeys.all, 'list'] as const,
  list: (state?: string) => [...jobsKeys.lists(), { state: state ?? null }] as const,
  detail: (id: string) => [...jobsKeys.all, 'detail', id] as const,
}
