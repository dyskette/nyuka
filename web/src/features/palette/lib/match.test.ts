import { describe, expect, it } from 'vitest'
import { matches, rank, rankBy } from './match'

describe('matches', () => {
  it('ignores case', () => {
    expect(matches('Ashfall Chronicle', 'ashfall')).toBe(true)
    expect(matches('ashfall chronicle', 'ASHFALL')).toBe(true)
  })

  /** An empty query showing nothing would make the palette useless to open. */
  it('matches everything on an empty query', () => {
    expect(matches('anything', '')).toBe(true)
  })

  it('does not match what is absent', () => {
    expect(matches('Ashfall Chronicle', 'zebra')).toBe(false)
  })
})

describe('rank', () => {
  /**
   * Three tiers, not a fuzzy score. Someone typing the first letters of a
   * title expects that title first; a fuzzy scorer that ranks it third is
   * worse than no ranking.
   */
  it('prefers exact, then prefix, then contains', () => {
    expect(rank('ash', 'ash')).toBe(0)
    expect(rank('Ashfall', 'ash')).toBe(1)
    expect(rank('Mountain Ash', 'ash')).toBe(2)
    expect(rank('Zebra', 'ash')).toBeNull()
  })
})

describe('rankBy', () => {
  it('orders by tier', () => {
    const titles = ['Mountain Ash', 'Ashfall Chronicle', 'ash']
    expect(rankBy(titles, 'ash', (t) => t)).toEqual(['ash', 'Ashfall Chronicle', 'Mountain Ash'])
  })

  /**
   * Equal matches keep the order the server sent, which for the library is
   * most-recently-updated first. A sort that reordered them would make the
   * palette disagree with the list behind it for no reason.
   */
  it('keeps the input order within a tier', () => {
    const titles = ['Ashes of Meridian', 'Ashfall Chronicle']
    expect(rankBy(titles, 'ash', (t) => t)).toEqual(['Ashes of Meridian', 'Ashfall Chronicle'])
  })

  it('drops what does not match', () => {
    expect(rankBy(['Alpha', 'Beta'], 'alp', (t) => t)).toEqual(['Alpha'])
  })

  it('keeps everything, in order, on an empty query', () => {
    expect(rankBy(['B', 'A'], '', (t) => t)).toEqual(['B', 'A'])
  })
})
