import { useMutation, useQueryClient } from '@tanstack/react-query'
import { libraryKeys } from '@/features/library/api/keys'
import { api } from '@/shared/api/client'
import { repoKeys, sourceKeys } from './keys'

/** Installs a source from a repository. */
export function useInstallSource() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ repoId, externalId }: { repoId: string; externalId: string }) => {
      const { data, error } = await api.POST('/sources', {
        body: { repo_id: repoId, external_id: externalId },
      })
      if (error !== undefined) throw error
      return data
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: sourceKeys.all })
      // The available list carries `installed_version` per entry, so it is
      // stale the moment anything is installed.
      void queryClient.invalidateQueries({ queryKey: repoKeys.all })
    },
  })
}

/**
 * Uninstalls a source.
 *
 * The library is invalidated too: the server cascades a source's series on
 * delete, so the library list is wrong the moment this returns. Leaving it
 * would show rows for a source that no longer exists until something else
 * happened to refetch.
 */
export function useUninstallSource() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (sourceId: string) => {
      const { error } = await api.DELETE('/sources/{id}', {
        params: { path: { id: sourceId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: sourceKeys.all })
      void queryClient.invalidateQueries({ queryKey: repoKeys.all })
      void queryClient.invalidateQueries({ queryKey: libraryKeys.all })
    },
  })
}

/**
 * Adds a repository.
 *
 * The name is the operator's label for it, not something read from the URL —
 * the index is not fetched until the repository exists, so there is nothing to
 * read a name from at this point.
 */
export function useAddRepo() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ url, name }: { url: string; name: string }) => {
      const { data, error } = await api.POST('/source-repos', { body: { url, name } })
      if (error !== undefined) throw error
      return data
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: repoKeys.all }),
  })
}

/** Removes a repository, and with it every source installed from it. */
export function useDeleteRepo() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (repoId: string) => {
      const { error } = await api.DELETE('/source-repos/{id}', {
        params: { path: { id: repoId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: repoKeys.all })
      void queryClient.invalidateQueries({ queryKey: sourceKeys.all })
      void queryClient.invalidateQueries({ queryKey: libraryKeys.all })
    },
  })
}

/** Re-reads a repository's index. */
export function useRefreshRepo() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (repoId: string) => {
      const { error } = await api.POST('/source-repos/{id}/refresh', {
        params: { path: { id: repoId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: repoKeys.all }),
  })
}

/**
 * Writes one of a source's settings.
 *
 * The value is already postcard-encoded and base64'd by the caller, which is
 * the only place that knows its type — the declaration says what each key
 * holds, and this server does not (see `lib/postcard.ts`).
 */
export function usePutSourceSetting(sourceId: string) {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ key, value }: { key: string; value: string }) => {
      const { error } = await api.PUT('/sources/{id}/settings', {
        params: { path: { id: sourceId } },
        body: { key, value },
      })
      if (error !== undefined) throw error
      return key
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: sourceKeys.settings(sourceId) }),
  })
}

/**
 * Adds a catalog entry to the library.
 *
 * The body names a source and a key, never the metadata: the server fetches
 * that from the source. Sending a title from here would let a client write
 * anything into the library.
 */
export function useAddToLibrary(sourceId: string) {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (externalKey: string) => {
      const { data, error } = await api.POST('/manga', {
        body: { source_id: sourceId, external_key: externalKey },
      })
      if (error !== undefined) throw error
      return data
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: libraryKeys.all })
      // The catalog carries `manga_id` per entry, which is what decides
      // whether a card offers "add" or "open".
      void queryClient.invalidateQueries({ queryKey: sourceKeys.catalogs(sourceId) })
    },
  })
}
