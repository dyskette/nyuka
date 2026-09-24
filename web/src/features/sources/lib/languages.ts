/**
 * How a source's languages are shown in a list.
 *
 * A source declares whatever it supports, and some declare a great many: the
 * community index has one with 40. Joined into a row they are not information
 * — nobody reads forty two-letter codes — and they are wide enough to push
 * everything else out of the row, which is what they did.
 */

/** How many are shown before the rest become a count. */
const SHOWN = 2

/**
 * A short label, with the remainder as a count.
 *
 * Truncating with an ellipsis instead would say only that something was cut;
 * a count says how much, which is the part that tells a reader whether to
 * look closer.
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
