import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

/**
 * The shell owns the viewport height; a screen fills the cell it is given.
 *
 * `AppShell` is `grid h-dvh grid-rows-[1fr_auto]` — the screen, then the
 * status bar — so a screen setting `h-dvh` overhangs its cell by the status
 * bar's height and `overflow-hidden` cuts that strip off.
 *
 * Asserted over the source because nothing in jsdom lays out.
 */
describe('a screen', () => {
  const dir = join(import.meta.dirname ?? __dirname, '.')
  // `-` prefixed files are excluded from the generated route tree, which is
  // how this test sits beside the screens it reads without becoming a route.
  const screens = readdirSync(dir).filter(
    (name) => /\.tsx$/.test(name) && !name.startsWith('-') && name !== '__root.tsx',
  )

  it('has screens to check', () => {
    expect(screens.length).toBeGreaterThan(3)
  })

  it.each(screens)('does not set the viewport height itself (%s)', (name) => {
    const source = readFileSync(join(dir, name), 'utf8')
    expect(
      source.includes('h-dvh'),
      `${name} sets \`h-dvh\`. That is the whole window; a screen gets the window ` +
        'less the status bar, so it would overhang its cell and be clipped. Use `h-full`.',
    ).toBe(false)
  })
})
