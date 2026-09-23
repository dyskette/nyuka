import { describe, expect, it } from 'vitest'
import { relativeTime } from './time'

const NOW = new Date('2026-09-23T12:00:00Z')

/**
 * The locale is always explicit here. Leaving it to the default would make
 * these assertions depend on the machine's language — they failed on a
 * Spanish host first, which is how the function's own reliance on the system
 * locale was found.
 */
function ago(ms: number, locale = 'en'): string {
  return relativeTime(new Date(NOW.getTime() - ms).toISOString(), { now: NOW, locale })
}

describe('relativeTime', () => {
  it('describes minutes, hours and days', () => {
    expect(ago(5 * 60_000)).toBe('5 minutes ago')
    expect(ago(3 * 60 * 60_000)).toBe('3 hours ago')
    expect(ago(2 * 24 * 60 * 60_000)).toBe('2 days ago')
  })

  /**
   * A freshness column that runs ahead of reality is worse than one that
   * lags: it claims a check happened longer ago than it did.
   */
  it('truncates rather than rounds up', () => {
    expect(ago(2 * 60 * 60_000 + 59 * 60_000)).toBe('2 hours ago')
  })

  it('uses words for the nearest values', () => {
    // `numeric: 'auto'` gives "yesterday" rather than "1 day ago".
    expect(ago(24 * 60 * 60_000)).toBe('yesterday')
  })

  /**
   * The reason the locale is a parameter at all: the same instant has to read
   * correctly inside a Spanish page.
   */
  it('formats in the locale it is given', () => {
    expect(ago(5 * 60_000, 'es')).toBe('hace 5 minutos')
    expect(ago(24 * 60 * 60_000, 'es')).toBe('ayer')
  })

  it('falls back to the smallest unit for anything very recent', () => {
    expect(ago(1_000)).toBe('this minute')
  })

  it('returns nothing for a value that is not a date', () => {
    expect(relativeTime('not a date', { now: NOW, locale: 'en' })).toBe('')
  })
})
