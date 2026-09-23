import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/**
 * Every operation the server serves is reached by the frontend, or is listed
 * below with a reason.
 *
 * Three endpoints were served with nothing reaching them and nothing saying
 * so: the Browse detail panel, the Downloads detail panel, and retrieving a
 * chapter file. TODO.md did not catch it because it tracked *screens*, and
 * every resource having a screen was mistaken for every operation having one.
 *
 * This is that check, run every time rather than when someone thinks to.
 */

// The `web/` directory, from `web/src/shared/api`.
const ROOT = resolve(__dirname ?? '.', '../../..')

/**
 * Operations nothing calls through the typed client, and why that is correct.
 *
 * An entry here is a claim someone can check, which is the point of listing
 * them rather than lowering the count.
 */
const EXPECTED_UNREACHED: Record<string, string> = {
  'get /chapters/{id}': [
    'Redundant. `/manga/{id}/chapters` returns every field this does, so no',
    'screen has a reason to ask for one chapter on its own.',
  ].join(' '),
  'get /downloads/{chapter_id}/file': [
    'Reached, but not through the typed client: it is a plain `<a href>` so the',
    "browser's own download handles it — ranged and streamed, rather than",
    'pulled into memory as a blob. Searched for separately below.',
  ].join(' '),
}

function sources(): string {
  const out: string[] = []
  const walk = (dir: string) => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry)
      if (statSync(path).isDirectory()) walk(path)
      else if (/\.tsx?$/.test(entry) && !entry.includes('.test.')) {
        out.push(readFileSync(path, 'utf8'))
      }
    }
  }
  walk(join(ROOT, 'src'))
  return out.join('\n')
}

describe('the API surface', () => {
  const spec = JSON.parse(readFileSync(join(ROOT, 'openapi.json'), 'utf8')) as {
    paths: Record<string, Record<string, unknown>>
  }
  const src = sources()

  const unreached: string[] = []
  for (const [path, operations] of Object.entries(spec.paths)) {
    for (const method of Object.keys(operations)) {
      if (!['get', 'post', 'put', 'delete', 'patch'].includes(method)) continue
      // The typed client is called with the literal path template.
      if (!src.includes(`'${path}'`)) unreached.push(`${method} ${path}`)
    }
  }

  it('is reached by the frontend, or says why not', () => {
    const unexplained = unreached.filter((operation) => !(operation in EXPECTED_UNREACHED))
    expect(
      unexplained,
      `these operations are served and nothing reaches them:\n  ${unexplained.join('\n  ')}\n\n` +
        'Build the UI, or add it to EXPECTED_UNREACHED with the reason.',
    ).toEqual([])
  })

  /**
   * The other direction. An entry that becomes reachable should be removed,
   * or the list quietly becomes a place where coverage goes to be forgotten.
   */
  it('has no stale exemptions', () => {
    const stale = Object.keys(EXPECTED_UNREACHED).filter(
      (operation) => !unreached.includes(operation),
    )
    expect(stale, 'these are reached now and no longer need an exemption').toEqual([])
  })

  /** The one exemption that claims to be reached another way has to be. */
  it('links to the chapter file outside the typed client', () => {
    expect(src).toMatch(/\/api\/v1\/downloads\/\$\{[^}]+\}\/file/)
  })
})
