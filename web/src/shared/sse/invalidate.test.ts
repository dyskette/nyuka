import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'
import { libraryKeys } from '@/features/library/api/keys'
import { INVALIDATES, applyEvent, invalidateEverything } from './invalidate'

function spyClient() {
  const queryClient = new QueryClient()
  const spy = vi.spyOn(queryClient, 'invalidateQueries').mockReturnValue(Promise.resolve())
  return { queryClient, spy }
}

describe('the invalidation table', () => {
  /**
   * A download job emits a progress tick per page. Invalidating the library
   * on each one would refetch it dozens of times per chapter — the exact
   * traffic ADR-0010 chose SSE to avoid.
   */
  it('does not invalidate anything on a progress tick', () => {
    expect(INVALIDATES['job.progress']({ job_id: 'j', done: 1, total: 9, bytes: 0 })).toEqual([])
  })

  /**
   * The reason `ChapterDownloaded` carries `manga_id` at all: without it the
   * only correct reaction is to refetch every series.
   */
  it('invalidates one series and the list when a chapter lands', () => {
    const keys = INVALIDATES['chapter.downloaded']({ manga_id: 'm-1', chapter_id: 'c-1' })

    expect(keys).toContainEqual(libraryKeys.detail('m-1'))
    expect(keys).not.toContainEqual(libraryKeys.detail('m-2'))
  })

  it('applies an event through the client', () => {
    const { queryClient, spy } = spyClient()

    applyEvent(queryClient, 'chapter.new', { manga_id: 'm-1', chapter_id: 'c-1' })

    expect(spy).toHaveBeenCalledWith({ queryKey: libraryKeys.detail('m-1') })
  })

  /**
   * A server ahead of this build emits names this table has never heard of.
   * Throwing inside an `EventSource` listener would kill the stream for every
   * other event, so an unknown name is a no-op.
   */
  it('ignores an event it does not know', () => {
    const { queryClient, spy } = spyClient()

    // @ts-expect-error — deliberately outside the union, which is the case
    // this covers: a server newer than this build.
    applyEvent(queryClient, 'chapter.deleted', { manga_id: 'm-1' })

    expect(spy).not.toHaveBeenCalled()
  })
})

/**
 * Reconnection is where correctness comes from — the client cannot know what
 * it missed, so it assumes it missed everything (ADR-0010). A narrower key
 * here would leave whatever it did not name stale until something else
 * happened to touch it.
 */
describe('invalidateEverything', () => {
  it('invalidates the whole feature, not one list', () => {
    const { queryClient, spy } = spyClient()

    invalidateEverything(queryClient)

    expect(spy).toHaveBeenCalledWith({ queryKey: libraryKeys.all })
    expect(libraryKeys.lists()).toEqual(expect.arrayContaining([...libraryKeys.all]))
  })
})
