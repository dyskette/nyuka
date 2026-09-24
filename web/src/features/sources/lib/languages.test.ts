import { describe, expect, it } from 'vitest'
import { languageLabel, languageTitle } from './languages'

describe('languageLabel', () => {
  it('shows a short list in full', () => {
    expect(languageLabel(['en'])).toBe('en')
    expect(languageLabel(['en', 'es'])).toBe('en, es')
  })

  /**
   * The case that broke the layout: one source in the community index
   * declares 40 languages, and joined into a row they took the whole width
   * and left no room for the name.
   */
  it('counts the rest rather than listing them', () => {
    const many = ['en', 'sq', 'ar', 'az', 'bn', 'bg', 'my', 'ca']
    expect(languageLabel(many)).toBe('en, sq +6')
  })

  it('says nothing when a source declares nothing', () => {
    expect(languageLabel([])).toBe('')
  })

  /** A label that grows with the list is the bug, so this bounds it. */
  it('has a bounded length whatever it is given', () => {
    const forty = Array.from({ length: 40 }, (_, index) => `l${index}`)
    expect(languageLabel(forty).length).toBeLessThan(20)
  })
})

describe('languageTitle', () => {
  it('carries the full list when the label is short of it', () => {
    expect(languageTitle(['en', 'es', 'fr'])).toBe('en, es, fr')
  })

  /** No tooltip that repeats what is already on screen. */
  it('is absent when the label already shows everything', () => {
    expect(languageTitle(['en'])).toBeUndefined()
    expect(languageTitle(['en', 'es'])).toBeUndefined()
    expect(languageTitle([])).toBeUndefined()
  })
})
