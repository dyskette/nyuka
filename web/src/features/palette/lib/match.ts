/**
 * Matching for the command palette.
 *
 * `shouldFilter={false}` is set on `cmdk` and this is used instead, for two
 * reasons: the palette renders a group of actions that must stay visible
 * whatever is typed, and the order of results is meaningful — a series whose
 * title starts with the query belongs above one that merely contains it.
 */

/**
 * Whether `haystack` matches `needle`.
 *
 * Case-insensitive through `toLocaleLowerCase`, because the plain
 * `toLowerCase` gets Turkish dotted and dotless I wrong. An empty query
 * matches everything, which is what makes the palette useful before typing.
 */
export function matches(haystack: string, needle: string): boolean {
  if (needle === '') return true
  return haystack.toLocaleLowerCase().includes(needle.toLocaleLowerCase())
}

/**
 * How well `haystack` matches, lower being better. `null` means no match.
 *
 * Three tiers rather than a fuzzy score: an exact match, then a prefix, then
 * anything containing it. A reader typing the first letters of a title expects
 * that title first, and a fuzzy scorer that ranks it third is worse than no
 * ranking at all.
 */
export function rank(haystack: string, needle: string): number | null {
  if (needle === '') return 0

  const hay = haystack.toLocaleLowerCase()
  const bit = needle.toLocaleLowerCase()

  if (hay === bit) return 0
  if (hay.startsWith(bit)) return 1
  if (hay.includes(bit)) return 2
  return null
}

/** Keeps what matches, best first, stable within a tier. */
export function rankBy<T>(items: T[], needle: string, key: (item: T) => string): T[] {
  return (
    items
      .map((item, index) => ({ item, index, score: rank(key(item), needle) }))
      .filter((entry): entry is { item: T; index: number; score: number } => entry.score !== null)
      // The original index breaks ties, so equal matches keep the order the
      // server sent — which for the library is most-recently-updated first.
      .sort((a, b) => a.score - b.score || a.index - b.index)
      .map((entry) => entry.item)
  )
}
