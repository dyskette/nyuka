import { queryOptions } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { repoKeys, sourceKeys } from './keys'

function unwrap<D, E>({ data, error }: { data?: D; error?: E }): D {
  if (error !== undefined) throw error
  return data as D
}

/** Installed sources. */
export function sourceListQuery() {
  return queryOptions({
    queryKey: sourceKeys.list(),
    queryFn: async ({ signal }) => unwrap(await api.GET('/sources', { signal })),
    // Installing is the only thing that changes this, and installing
    // invalidates it. Nothing else needs to refetch.
    staleTime: 5 * 60_000,
  })
}

/**
 * A source's catalog.
 *
 * `retry: false` unlike everything else: this request reaches out to a third
 * party, and a source that is down stays down for longer than three retries.
 * Failing once and saying so beats three timeouts before the same message.
 */
export function catalogQuery(sourceId: string, q?: string, cursor?: string) {
  return queryOptions({
    queryKey: sourceKeys.catalog(sourceId, q, cursor),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/sources/{id}/catalog', {
          params: {
            path: { id: sourceId },
            query: { ...(q ? { q } : {}), ...(cursor ? { cursor } : {}) },
          },
          signal,
        }),
      ),
    retry: false,
    // A catalog is the source's own listing and changes on its schedule, not
    // this server's. Long enough that paging back does not refetch.
    staleTime: 5 * 60_000,
  })
}

/**
 * A source's settings: its declaration and its stored values.
 *
 * `staleTime: 0` — this is the one view whose whole purpose is to show what is
 * currently stored, and a save invalidates it.
 */
export function sourceSettingsQuery(sourceId: string) {
  return queryOptions({
    queryKey: sourceKeys.settings(sourceId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/sources/{id}/settings', {
          params: { path: { id: sourceId } },
          signal,
        }),
      ),
    staleTime: 0,
  })
}

/** Configured repositories. */
export function repoListQuery() {
  return queryOptions({
    queryKey: repoKeys.list(),
    queryFn: async ({ signal }) => unwrap(await api.GET('/source-repos', { signal })),
    staleTime: 5 * 60_000,
  })
}

/** What a repository offers, with `installed_version` on the ones already in. */
export function availableSourcesQuery(repoId: string) {
  return queryOptions({
    queryKey: repoKeys.available(repoId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET('/source-repos/{id}/available', {
          params: { path: { id: repoId } },
          signal,
        }),
      ),
    staleTime: 5 * 60_000,
  })
}
