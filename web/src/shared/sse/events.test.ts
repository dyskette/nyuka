import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { EVENT_NAMES, LAGGED_EVENT, parseEvent } from './events'

/**
 * The event names are hand-written on this side, because an SSE stream's
 * names and payloads are in no OpenAPI document. This reads the server's own
 * list so a rename on either side fails here rather than producing a listener
 * that is never called — which is silent in every other test and in
 * production.
 */
describe('the event contract', () => {
  // A path from the Vitest root rather than `import.meta.url`: under jsdom
  // that is an `http://localhost/` URL, not a file one.
  const sse = readFileSync(resolve(__dirname ?? '.', '../../../../crates/api/src/sse.rs'), 'utf8')

  it('lists exactly the names the server emits', () => {
    // The `event_name` match arms, e.g. `JobEvent::JobProgress { .. } => "job.progress",`
    const served = [...sse.matchAll(/JobEvent::\w+ \{ \.\. \} => "([\w.]+)"/g)].map((m) => m[1])

    expect(served.length).toBeGreaterThan(0)
    expect([...EVENT_NAMES].sort()).toEqual(served.toSorted())
  })

  it('uses the server name for a lagged stream', () => {
    const declared = /LAGGED_EVENT: &str = "([\w.]+)"/.exec(sse)?.[1]
    expect(LAGGED_EVENT).toBe(declared)
  })
})

describe('parseEvent', () => {
  /**
   * The server tags internally, so the fields are flat and `event` rides
   * along beside them. Reading a nested object would produce `undefined` ids
   * and invalidate nothing.
   */
  it('reads the flat payload the server sends', () => {
    const payload = parseEvent<'chapter.downloaded'>(
      '{"event":"chapter_downloaded","manga_id":"m-1","chapter_id":"c-1"}',
    )

    expect(payload).toEqual({
      event: 'chapter_downloaded',
      manga_id: 'm-1',
      chapter_id: 'c-1',
    })
  })

  /**
   * A malformed payload must not throw: the throw would escape an
   * `EventSource` listener and take every other event with it.
   */
  it('returns null rather than throwing on a payload that is not JSON', () => {
    expect(parseEvent('not json')).toBeNull()
    expect(parseEvent('null')).toBeNull()
    expect(parseEvent('42')).toBeNull()
  })
})
