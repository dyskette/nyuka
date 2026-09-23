/**
 * The event contract, mirrored from `crates/api/src/sse.rs`.
 *
 * Hand-written rather than generated: the OpenAPI document describes request
 * and response bodies, and an SSE stream's event names and payloads are in
 * neither. `src/shared/sse/events.test.ts` checks these names against the
 * server's own list, so a rename on either side is a test failure rather than
 * a listener that never fires.
 */

export const EVENT_NAMES = [
  'job.progress',
  'job.state',
  'chapter.new',
  'chapter.downloaded',
  'source.updated',
] as const

export type EventName = (typeof EVENT_NAMES)[number]

/** The server's own name for "you fell behind and missed events". */
export const LAGGED_EVENT = 'lagged'

export interface JobProgress {
  job_id: string
  done: number
  total: number
  bytes: number
}

export interface JobStateChanged {
  job_id: string
  state: string
}

export interface ChapterNew {
  manga_id: string
  chapter_id: string
}

export interface ChapterDownloaded {
  manga_id: string
  chapter_id: string
}

export interface SourceUpdated {
  source_id: string
}

export interface EventPayloads {
  'job.progress': JobProgress
  'job.state': JobStateChanged
  'chapter.new': ChapterNew
  'chapter.downloaded': ChapterDownloaded
  'source.updated': SourceUpdated
}

/**
 * Parses one event payload.
 *
 * The server tags its enum *internally* — `{"event":"chapter_downloaded",
 * "manga_id":…}` — so the fields are already flat and the `event` field is a
 * snake_case duplicate of the SSE event name, which is dotted. Neither is
 * used for dispatch: `addEventListener` already did that, and trusting the
 * body over the listener would mean a mismatched pair silently routes to the
 * wrong handler.
 */
export function parseEvent<N extends EventName>(raw: string): EventPayloads[N] | null {
  try {
    const parsed: unknown = JSON.parse(raw)
    if (typeof parsed !== 'object' || parsed === null) return null
    return parsed as EventPayloads[N]
  } catch {
    // A payload that does not parse is a server bug, and dropping the event is
    // safe: reconnection invalidates everything anyway (ADR-0010).
    return null
  }
}
