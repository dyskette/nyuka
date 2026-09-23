import { expect, test } from '@playwright/test'

/**
 * The contrast test ADR-0016 calls load-bearing: over *rendered* colours, in
 * both themes, including the focus ring.
 *
 * # Why the token table in `theme.css` is not this
 *
 * Those comments are a first pass measured by hand, and the ADR says two
 * things that make them insufficient on their own:
 *
 * 1. OKLCH lightness is not WCAG luminance. Three semantic hues at the same
 *    `L` measured 4.46, 4.95 and 5.38 against white.
 * 2. Five tokens fall outside sRGB and are gamut-mapped by the browser, so the
 *    rendered colour is not the specified one and its contrast is not the
 *    computed one.
 *
 * Four tokens were already corrected after measurement found them failing,
 * including a focus ring at 1.92:1 against the 3:1 that WCAG 1.4.11 requires —
 * and which measured 4.63:1 in dark mode, which is exactly how it would have
 * shipped unnoticed.
 *
 * # How a colour is measured
 *
 * Painted to a canvas and read back. That returns the sRGB the browser
 * actually produces, gamut mapping included — `oklch(70% 0.16 262)` comes back
 * as `102,155,255`, clamped at the blue channel. `getComputedStyle` would
 * return the `oklch()` string unchanged and measure nothing.
 */

type Rgb = [number, number, number]

/** WCAG 2.2 relative luminance. */
function luminance([r, g, b]: Rgb): number {
  const channel = (value: number) => {
    const c = value / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

function ratio(a: Rgb, b: Rgb): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return ((light as number) + 0.05) / ((dark as number) + 0.05)
}

/**
 * Measures one pair as it is actually painted.
 *
 * The foreground is composited **over its own background**, not over black.
 * That distinction is the whole test: a token with alpha over a light surface
 * is lighter and contrasts *less*, while over black it darkens and contrasts
 * more. Measuring over black reports a passing number for the exact defect
 * ADR-0016 found — the focus ring at 60% alpha, which measured 1.92:1 in light
 * mode and 4.63:1 in dark.
 *
 * The first version of this test did composite over black, and did not catch
 * that ring when it was put back.
 *
 * Backgrounds are read on their own because they are opaque; if one ever gains
 * alpha it would need the same treatment against whatever is behind it.
 */
async function measure(
  page: import('@playwright/test').Page,
  theme: 'light' | 'dark',
  front: string,
  back: string,
): Promise<{ front: Rgb; back: Rgb }> {
  return page.evaluate(
    ({ mode, frontName, backName }) => {
      document.documentElement.setAttribute('data-theme', mode)

      const canvas = document.createElement('canvas')
      canvas.width = 1
      canvas.height = 1
      const ctx = canvas.getContext('2d')
      if (ctx === null) throw new Error('no 2d context')

      const style = getComputedStyle(document.documentElement)
      const css = (name: string) => {
        const value = style.getPropertyValue(`--${name}`).trim()
        if (value === '') throw new Error(`--${name} is not defined`)
        return value
      }

      const read = (): [number, number, number] => {
        const data = ctx.getImageData(0, 0, 1, 1).data
        return [data[0] as number, data[1] as number, data[2] as number]
      }

      // The background alone. Painted over white so an unexpected alpha shows
      // as a lighter colour rather than silently compositing against nothing.
      ctx.fillStyle = '#ffffff'
      ctx.fillRect(0, 0, 1, 1)
      ctx.fillStyle = css(backName)
      ctx.fillRect(0, 0, 1, 1)
      const background = read()

      // The foreground on top of it, which is what the eye sees.
      ctx.fillStyle = css(frontName)
      ctx.fillRect(0, 0, 1, 1)
      const foreground = read()

      return { front: foreground, back: background }
    },
    { mode: theme, frontName: front, backName: back },
  )
}

/**
 * The pairs, and what each has to clear.
 *
 * 4.5 for anything rendered as normal-size text (WCAG 1.4.3), 3.0 for anything
 * that is a user-interface component or a meaningful graphic (1.4.11).
 */
const TEXT = 4.5
const NON_TEXT = 3

const PAIRS: [string, string, number, string][] = [
  ['foreground', 'background', TEXT, 'body text'],
  ['foreground', 'surface', TEXT, 'text on a panel'],
  ['muted-foreground', 'background', TEXT, 'secondary text'],
  ['muted-foreground', 'surface-raised', TEXT, 'secondary text on a raised row'],
  ['accent', 'background', TEXT, 'a link'],
  ['accent-foreground', 'accent', TEXT, 'text on an accent fill'],
  ['success', 'background', TEXT, 'a success message'],
  ['warning', 'background', TEXT, 'a warning message'],
  ['danger', 'background', TEXT, 'an error message'],
  // The one that was 1.92:1 and passed review. WCAG 2.4.11 and 1.4.11: a
  // focus indicator is a component boundary, not text.
  ['ring', 'background', NON_TEXT, 'the focus ring over the page'],
  ['ring', 'surface', NON_TEXT, 'the focus ring over a panel'],
  ['ring', 'surface-raised', NON_TEXT, 'the focus ring over a hovered row'],
  // Semantic dots carry meaning beside their label, so they are graphics.
  ['success', 'surface-raised', NON_TEXT, 'a success dot'],
  ['danger', 'surface-raised', NON_TEXT, 'a danger dot'],
]

for (const theme of ['light', 'dark'] as const) {
  test.describe(`${theme} theme`, () => {
    test('every colour pair clears its WCAG threshold', async ({ page }) => {
      await page.goto('/')

      const failures: string[] = []
      for (const [front, back, minimum, what] of PAIRS) {
        const painted = await measure(page, theme, front, back)
        const measured = ratio(painted.front, painted.back)
        if (measured < minimum) {
          failures.push(
            `${what}: --${front} on --${back} is ${measured.toFixed(2)}:1, needs ${minimum}:1`,
          )
        }
      }

      // Every failure at once. Reporting the first would mean one run per
      // broken token, and these are usually broken together — a hue shifted
      // across a whole palette fails in three places.
      expect(failures, `${theme} theme contrast failures:\n  ${failures.join('\n  ')}`).toEqual([])
    })
  })
}

/**
 * The reason the pairs above are not the whole test: they check tokens against
 * the backgrounds they are *meant* to appear on. A component that puts one
 * somewhere else is a failure no token table can see.
 */
test('the rendered shell has no contrast violations', async ({ page }) => {
  const { default: AxeBuilder } = await import('@axe-core/playwright')

  await page.goto('/')
  const results = await new AxeBuilder({ page }).withRules(['color-contrast']).analyze()

  const described = results.violations.flatMap((violation) =>
    violation.nodes.map((node) => `${node.target.join(' ')}: ${node.failureSummary ?? ''}`),
  )
  expect(described, `contrast violations:\n  ${described.join('\n  ')}`).toEqual([])
})
