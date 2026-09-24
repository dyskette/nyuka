import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

/**
 * The shell owns the viewport height; a screen fills the cell it is given.
 *
 * `AppShell` is `grid h-dvh grid-rows-[1fr_auto]` — the screen, then the
 * status bar. Every screen also set `h-dvh`, which is the whole window rather
 * than the window less the status bar, so each one overhung its cell by
 * exactly the status bar's height and `overflow-hidden` cut that strip off.
 *
 * It showed as the "Next page" button in Browse being sliced in half at the
 * bottom of the catalog, but it was every screen: the last row of the
 * library, the last job in the queue, the end of the settings column.
 *
 * Nothing in jsdom lays out, so this is asserted over the source. A screen
 * that needs a viewport-height element inside itself should say so here.
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
