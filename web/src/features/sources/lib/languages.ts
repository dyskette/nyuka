/**
 * How a source's languages are shown in a list.
 *
 * Some declare 40. Joined into a row they are neither readable nor narrow
 * enough to share it.
 */

/** How many are shown before the rest become a count. */
const SHOWN = 2

/**
 * A short label, with the remainder as a count.
 *
 * A count says how much was left out, where an ellipsis says only that
 * something was.
 */
export function languageLabel(languages: string[]): string {
  if (languages.length === 0) return ''
  if (languages.length <= SHOWN) return languages.join(', ')

  return `${languages.slice(0, SHOWN).join(', ')} +${languages.length - SHOWN}`
}

/**
 * The full list, for a `title` attribute.
 *
 * Returns `undefined` when the label already shows everything, so a row does
 * not carry a tooltip that repeats what is on screen.
 */
export function languageTitle(languages: string[]): string | undefined {
  return languages.length > SHOWN ? languages.join(', ') : undefined
}
