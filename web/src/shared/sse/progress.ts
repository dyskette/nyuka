import { useSyncExternalStore } from 'react'
import type { JobProgress } from './events'

/**
 * Live download progress, keyed by job id.
 *
 * Not in the Query cache, because there is nothing to fetch: `job.progress` is
 * emitted while a job runs and the server exposes no endpoint that reports it.
 * A cache entry with no `queryFn` would look refetchable and never be.
 *
 * So this is what it is — ephemeral client state, lost on reload, correct only
 * while connected. A progress bar that disappears on refresh is honest; one
 * restored from a stale number is not.
 *
 * A module singleton rather than a context value: there is exactly one
 * `EventSource` (ADR-0010), so there is exactly one writer.
 */
export interface Progress {
  done: number
  total: number
  bytes: number
  /** Bytes per second over the last interval, once two samples exist. */
  bytesPerSecond: number | null
}

interface Sample {
  bytes: number
  at: number
}

const progress = new Map<string, Progress>()
const lastSample = new Map<string, Sample>()
const listeners = new Set<() => void>()

/** The snapshot `useSyncExternalStore` compares by reference. */
let snapshot: ReadonlyMap<string, Progress> = new Map()

function publish() {
  // A fresh Map each time: `useSyncExternalStore` bails out when the
  // reference is unchanged, so mutating in place would render nothing.
  snapshot = new Map(progress)
  for (const listener of listeners) listener()
}

export function recordProgress(event: JobProgress, now: number = Date.now()): void {
  const previous = lastSample.get(event.job_id)
  const elapsed = previous ? (now - previous.at) / 1000 : 0

  // Guarded on both sides: a zero interval divides by zero, and a byte count
  // that went backwards is a restarted attempt rather than negative speed.
  const bytesPerSecond =
    previous && elapsed > 0 && event.bytes >= previous.bytes
      ? (event.bytes - previous.bytes) / elapsed
      : null

  progress.set(event.job_id, {
    done: event.done,
    total: event.total,
    bytes: event.bytes,
    bytesPerSecond,
  })
  lastSample.set(event.job_id, { bytes: event.bytes, at: now })
  publish()
}

/**
 * Drops a job's progress once it reaches a terminal state.
 *
 * Without this the map grows for the life of the tab, and a finished job keeps
 * rendering the bar it had when it stopped.
 */
export function clearProgress(jobId: string): void {
  if (!progress.delete(jobId)) return
  lastSample.delete(jobId)
  publish()
}

/** Test-only: forget everything, so one test cannot see another's events. */
export function resetProgress(): void {
  progress.clear()
  lastSample.clear()
  publish()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function getSnapshot(): ReadonlyMap<string, Progress> {
  return snapshot
}

/** Live progress for every running job. */
export function useProgress(): ReadonlyMap<string, Progress> {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
