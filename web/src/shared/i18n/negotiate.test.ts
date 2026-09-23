import { describe, expect, it } from 'vitest'
import { DEFAULT_LOCALE, negotiate } from './index'

describe('locale negotiation', () => {
  it('matches an exact locale', () => {
    expect(negotiate(['es'])).toBe('es')
    expect(negotiate(['en'])).toBe('en')
  })

  /**
   * A reader in Mexico getting English because the region did not match
   * exactly is the failure this exists to avoid.
   */
  it('matches on the language subtag, ignoring the region', () => {
    expect(negotiate(['es-MX'])).toBe('es')
    expect(negotiate(['es-419'])).toBe('es')
    expect(negotiate(['en-GB'])).toBe('en')
  })

  it('takes the first supported preference, not the first preference', () => {
    expect(negotiate(['de', 'fr', 'es', 'en'])).toBe('es')
  })

  it('falls back when nothing is supported', () => {
    expect(negotiate(['de', 'fr'])).toBe(DEFAULT_LOCALE)
    expect(negotiate([])).toBe(DEFAULT_LOCALE)
  })

  it('is case insensitive', () => {
    expect(negotiate(['ES-mx'])).toBe('es')
  })
})
