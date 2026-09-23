import { useMutation, useQueryClient } from '@tanstack/react-query'
import { api } from '@/shared/api/client'
import { jobKeys } from './keys'

/** Cancels a queued or running job. */
export function useCancelJob() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (jobId: string) => {
      const { error } = await api.POST('/jobs/{id}/cancel', {
        params: { path: { id: jobId } },
      })
      if (error !== undefined) throw error
    },
    // On settle rather than on success: a 409 means the job already reached a
    // terminal state, and the cache is what is wrong in that case.
    onSettled: () => queryClient.invalidateQueries({ queryKey: jobKeys.all }),
  })
}

/** Puts a failed job back in the queue. */
export function useRetryJob() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (jobId: string) => {
      const { error } = await api.POST('/jobs/{id}/retry', {
        params: { path: { id: jobId } },
      })
      if (error !== undefined) throw error
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: jobKeys.all }),
  })
}
