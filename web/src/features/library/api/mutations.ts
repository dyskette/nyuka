import { useMutation, useQueryClient } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { libraryKeys } from './keys'

/**
 * Requests a chapter download.
 *
 * The server returns 202 and a `Location` naming the job — the file does not
 * exist yet. So this does not write an optimistic "downloaded" into the
 * cache: the honest local state is "queued", and the real one arrives over
 * SSE when the job finishes (ADR-0010).
 *
 * The chapter list is invalidated on settle rather than on success, because a
 * 409 means the chapter is already in the library — the cache is what is
 * wrong in that case, and refetching is exactly the right response.
 */
export function useRequestDownload(mangaId: string) {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (chapterId: string) => {
      const { error } = await api.POST('/downloads', {
        body: { chapter_id: chapterId },
        headers: {
          // Scoped to the chapter, so a double click is one download while
          // two different chapters remain two. A random key per press would
          // make the header decorative.
          'Idempotency-Key': `download:${chapterId}`,
        },
      })
      if (error !== undefined) throw error
      return chapterId
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: libraryKeys.detail(mangaId) }),
  })
}

/**
 * Removes a series from the library.
 *
 * `files` asks the server to delete what was downloaded of it as well. The
 * default is to keep them: the row is the library's record of a series and
 * the files are the reader's copy, and only one of those is cheap to get
 * back — so the caller has to say.
 */
export function useRemoveFromLibrary() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async ({ mangaId, files }: { mangaId: string; files: boolean }) => {
      const { error } = await api.DELETE('/manga/{id}', {
        params: { path: { id: mangaId }, query: { files } },
      })
      if (error !== undefined) throw error
      return mangaId
    },
    // The whole feature: the list loses a row, and the panel is showing a
    // series that no longer exists.
    onSettled: () => queryClient.invalidateQueries({ queryKey: libraryKeys.all }),
  })
}
