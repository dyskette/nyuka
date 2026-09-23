import { useMutation, useQueryClient } from '@tanstack/react-query'
import { jobKeys } from '@/features/jobs/api/keys'
import { libraryKeys } from '@/features/library/api/keys'
import { api } from '@/shared/api/client'
import { followKeys } from './keys'

/**
 * Creates or updates a follow.
 *
 * `PUT` rather than `POST`: a follow is identified by the series it watches,
 * so following twice is the same follow with the later settings, not two.
 */
export function useUpsertFollow() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (body: {
      manga_id: string
      check_interval_secs?: number
      auto_download?: boolean
    }) => {
      const { data, error } = await api.PUT('/follows', { body })
      if (error !== undefined) throw error
      return data
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: followKeys.all }),
  })
}

/** Stops following a series. The series itself stays in the library. */
export function useUnfollow() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (followId: string) => {
      const { error } = await api.DELETE('/follows/{id}', {
        params: { path: { id: followId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: followKeys.all }),
  })
}

/**
 * Checks a follow now rather than waiting for its interval.
 *
 * The server answers 202 — it enqueues a job. So this invalidates the queue
 * as well as the follow list, and the result arrives over SSE rather than
 * from this response.
 */
export function useCheckFollowNow() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (followId: string) => {
      const { error } = await api.POST('/follows/{id}/check-now', {
        params: { path: { id: followId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: followKeys.all })
      void queryClient.invalidateQueries({ queryKey: jobKeys.all })
      void queryClient.invalidateQueries({ queryKey: libraryKeys.all })
    },
  })
}
