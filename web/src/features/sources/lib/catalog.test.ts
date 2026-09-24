import { describe, expect, it } from 'vitest'
import type { components } from '@/shared/api/schema'
import { groupByLanguage, languageName, MULTI, matches, ratingBadge } from './catalog'

type SourceEntry = components['schemas']['SourceEntryDto']

function entry(overrides: Partial<SourceEntry> = {}): SourceEntry {
  return {
    repo_id: 'r-1',
    external_id: 'en.example',
    name: 'Example',
    version: 1,
    icon_url: null,
    languages: ['en'],
    content_rating: 'safe',
    base_url: 'https://example.test',
    installed_version: null,
    ...overrides,
  }
}

describe('matches', () => {
  it('finds a source by name', () => {
    expect(matches(entry({ name: 'Asura Scans' }), 'asura')).toBe(true)
  })

  /** Several sources are named nothing like the site they read. */
  it('finds a source by its address', () => {
    expect(
      matches(entry({ name: 'Armageddon', base_url: 'https://silentquill.net' }), 'quill'),
    ).toBe(true)
  })

  /** A Spanish or Vietnamese name should not need its accents typed. */
  it('ignores diacritics', () => {
    expect(matches(entry({ name: 'Cứu Truyện' }), 'cuu truyen')).toBe(true)
  })

  it('keeps everything for an empty query', () => {
    expect(matches(entry(), '   ')).toBe(true)
  })

  it('rejects what does not match', () => {
    expect(matches(entry({ name: 'BatCave', base_url: null }), 'asura')).toBe(false)
  })
})

describe('groupByLanguage', () => {
  const label = (code: string) => (code === MULTI ? 'Multi' : languageName(code, 'en'))

  it('puts a single-language source under its language', () => {
    const groups = groupByLanguage([entry({ languages: ['en'] })], label)

    expect(groups).toHaveLength(1)
    expect(groups[0]?.code).toBe('en')
    expect(groups[0]?.label).toBe('English')
  })

  it('puts a source serving several languages under multi', () => {
    const groups = groupByLanguage([entry({ languages: ['en', 'es'] })], label)
    expect(groups[0]?.code).toBe(MULTI)
  })

  /** As unplaceable as one declaring nine. */
  it('puts a source declaring no language under multi', () => {
    const groups = groupByLanguage([entry({ languages: [] })], label)
    expect(groups[0]?.code).toBe(MULTI)
  })

  it('leads with multi, then sorts by the displayed name', () => {
    const groups = groupByLanguage(
      [
        entry({ external_id: 'a', languages: ['fr'] }),
        entry({ external_id: 'b', languages: ['en'] }),
        entry({ external_id: 'c', languages: ['en', 'ja'] }),
        entry({ external_id: 'd', languages: ['es'] }),
      ],
      label,
    )

    expect(groups.map((g) => g.label)).toEqual(['Multi', 'English', 'French', 'Spanish'])
  })

  /**
   * Sorted by the *name*, not the code, or a reader sees an order that is
   * alphabetical in a language they are not reading.
   */
  it('sorts by the name the reader sees', () => {
    const spanish = (code: string) => (code === MULTI ? 'Multi' : languageName(code, 'es'))
    // In Spanish: alemán, inglés — the reverse of the codes de, en.
    const groups = groupByLanguage(
      [
        entry({ external_id: 'a', languages: ['en'] }),
        entry({ external_id: 'b', languages: ['de'] }),
      ],
      spanish,
    )

    expect(groups.map((g) => g.code)).toEqual(['de', 'en'])
  })

  it('keeps the index order inside a group', () => {
    const groups = groupByLanguage(
      [entry({ external_id: 'a', name: 'Athrea' }), entry({ external_id: 'b', name: 'BatCave' })],
      label,
    )

    expect(groups[0]?.entries.map((e) => e.name)).toEqual(['Athrea', 'BatCave'])
  })
})

describe('languageName', () => {
  it('names a language in the reader own language', () => {
    expect(languageName('en', 'es')).toBe('inglés')
  })

  /** An index carries whatever a source author wrote; `Intl` throws on it. */
  it('falls back to the code it was given', () => {
    expect(languageName('All', 'en')).toBe('All')
  })
})

describe('ratingBadge', () => {
  it('badges the two that warn', () => {
    expect(ratingBadge('suggestive')).toBe('17+')
    expect(ratingBadge('nsfw')).toBe('18+')
  })

  /** A badge on almost every row makes the two that matter invisible. */
  it('leaves the rest unbadged', () => {
    expect(ratingBadge('safe')).toBeUndefined()
    expect(ratingBadge('unknown')).toBeUndefined()
  })
})
