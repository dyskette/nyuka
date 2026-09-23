import { renderHook } from '@testing-library/react'
import { act } from 'react'
import { beforeEach, describe, expect, it } from 'vitest'
import { clearProgress, recordProgress, resetProgress, useProgress } from './progress'

beforeEach(resetProgress)

function read() {
  return renderHook(() => useProgress()).result.current
}

describe('the progress store', () => {
  it('records what an event reported', () => {
    act(() => recordProgress({ job_id: 'j1', done: 3, total: 9, bytes: 100 }, 1_000))

    expect(read().get('j1')).toEqual({
      done: 3,
      total: 9,
      bytes: 100,
      // No speed from one sample: a rate needs an interval.
      bytesPerSecond: null,
    })
  })

  it('derives a rate from two samples', () => {
    act(() => {
      recordProgress({ job_id: 'j1', done: 1, total: 9, bytes: 1_000 }, 1_000)
      recordProgress({ job_id: 'j1', done: 2, total: 9, bytes: 3_000 }, 3_000)
    })

    // 2000 bytes over 2 seconds.
    expect(read().get('j1')?.bytesPerSecond).toBe(1_000)
  })

  /**
   * Two events in the same millisecond divide by zero. `Infinity B/s` in a
   * table is worse than no number.
   */
  it('reports no rate when no time passed', () => {
    act(() => {
      recordProgress({ job_id: 'j1', done: 1, total: 9, bytes: 1_000 }, 1_000)
      recordProgress({ job_id: 'j1', done: 2, total: 9, bytes: 2_000 }, 1_000)
    })

    expect(read().get('j1')?.bytesPerSecond).toBeNull()
  })

  /**
   * A retried attempt starts its byte count over. Subtracting would give a
   * negative rate, which is not a slower download — it is a different one.
   */
  it('reports no rate when the byte count went backwards', () => {
    act(() => {
      recordProgress({ job_id: 'j1', done: 8, total: 9, bytes: 9_000 }, 1_000)
      recordProgress({ job_id: 'j1', done: 1, total: 9, bytes: 100 }, 3_000)
    })

    expect(read().get('j1')?.bytesPerSecond).toBeNull()
  })

  it('keeps jobs apart', () => {
    act(() => {
      recordProgress({ job_id: 'j1', done: 1, total: 9, bytes: 1 }, 1_000)
      recordProgress({ job_id: 'j2', done: 5, total: 5, bytes: 50 }, 1_000)
    })

    expect(read().get('j1')?.done).toBe(1)
    expect(read().get('j2')?.done).toBe(5)
  })

  /**
   * Without this the map grows for the life of the tab, and a finished job
   * keeps rendering the bar it had when it stopped.
   */
  it('forgets a job that finished', () => {
    act(() => recordProgress({ job_id: 'j1', done: 9, total: 9, bytes: 900 }, 1_000))
    act(() => clearProgress('j1'))

    expect(read().has('j1')).toBe(false)
  })

  /**
   * `useSyncExternalStore` bails out when the snapshot is reference-equal, so
   * a store that mutated in place would update nothing on screen while every
   * direct read of it looked right.
   */
  it('publishes a new snapshot on each write', () => {
    const { result, rerender } = renderHook(() => useProgress())
    const before = result.current

    act(() => recordProgress({ job_id: 'j1', done: 1, total: 9, bytes: 1 }, 1_000))
    rerender()

    expect(result.current).not.toBe(before)
    expect(result.current.get('j1')?.done).toBe(1)
  })
})
