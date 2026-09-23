import { i18n } from '@lingui/core'

/**
 * Relative time, for columns where "how long ago" is the question.
 *
 * `Intl.RelativeTimeFormat` rather than a hand-rolled table: it is localised,
 * and it applies each locale's plural rules — Spanish pluralisation is not
 * English's with a different word.
 *
 * # The locale comes from the application, not the platform
 *
 * Passing `undefined` makes `Intl` use the system locale, which is not
 * necessarily the one the application is running in: a reader whose browser
 * is English but who chose Spanish would get "2 hours ago" inside an
 * otherwise Spanish page. Defaulting to the active catalog's locale keeps one
 * answer to "what language is this".
 */
const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['year', 365 * 24 * 60 * 60 * 1000],
  ['month', 30 * 24 * 60 * 60 * 1000],
  ['day', 24 * 60 * 60 * 1000],
  ['hour', 60 * 60 * 1000],
  ['minute', 60 * 1000],
]

export interface RelativeTimeOptions {
  now?: Date
  /** Defaults to the active catalog's locale. */
  locale?: string
}

export function relativeTime(iso: string, options: RelativeTimeOptions = {}): string {
  const then = new Date(iso)
  if (Number.isNaN(then.getTime())) return ''

  const now = options.now ?? new Date()
  const locale = options.locale ?? i18n.locale
  const elapsed = then.getTime() - now.getTime()
  const format = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' })

  for (const [unit, size] of UNITS) {
    if (Math.abs(elapsed) >= size) {
      // Truncated toward zero rather than rounded: "2 hours ago" for
      // something 2h59m old understates, but rounding up to 3 claims more
      // elapsed time than has passed, and a freshness column that runs ahead
      // of reality reads as the scheduler being behind.
      return format.format(Math.trunc(elapsed / size), unit)
    }
  }

  return format.format(0, 'minute')
}
