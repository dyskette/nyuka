/**
 * Query keys for the jobs feature (ADR-0009).
 *
 * Separate from the library's tree: a job finishing invalidates both, and one
 * shared root would make "invalidate the library" also throw away the queue
 * on every chapter that lands.
 */
export const jobKeys = {
  all: ['jobs'] as const,

  lists: () => [...jobKeys.all, 'list'] as const,
  /** Keyed on the state filter, because each filter is its own server query. */
  list: (state?: string) => [...jobKeys.lists(), { state: state ?? null }] as const,

  details: () => [...jobKeys.all, 'detail'] as const,
  detail: (jobId: string) => [...jobKeys.details(), jobId] as const,
}
