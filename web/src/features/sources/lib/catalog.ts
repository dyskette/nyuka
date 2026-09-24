import type { components } from '@/shared/api/schema'

type SourceEntry = components['schemas']['SourceEntryDto']

/** The group for a source that serves more than one language. */
export const MULTI = 'multi'

/** Folded for comparison, so `Cứu Truyện` answers to `cuu truyen`. */
function fold(value: string): string {
  return value
    .normalize('NFD')
    .replace(/\p{Diacritic}/gu, '')
    .toLocaleLowerCase()
}

/**
 * Whether an entry answers to what was typed.
 *
 * The site is matched as well as the name: a reader looking for a source
 * usually knows the address they read it at, and several sources are named
 * nothing like their domain.
 */
export function matches(entry: SourceEntry, query: string): boolean {
  const needle = fold(query.trim())
  if (needle === '') return true

  return fold(entry.name).includes(needle) || fold(entry.base_url ?? '').includes(needle)
}

export interface LanguageGroup {
  /** A language code, or [`MULTI`]. */
  code: string
  label: string
  entries: SourceEntry[]
}

/**
 * Entries under a heading per language, multi-language first.
 *
 * 136 sources in one alphabetical list is a list nobody reads to the end. The
 * language is the first thing a reader filters on, so it is the grouping
 * rather than a column.
 *
 * `labelFor` supplies the display name, because it is both translated and
 * locale-dependent and this stays free of both.
 */
export function groupByLanguage(
  entries: SourceEntry[],
  labelFor: (code: string) => string,
): LanguageGroup[] {
  const groups = new Map<string, SourceEntry[]>()

  for (const entry of entries) {
    // A source declaring none is as unplaceable as one declaring nine, and
    // both belong under the heading that promises nothing.
    const code = entry.languages.length === 1 ? (entry.languages[0] as string) : MULTI
    const bucket = groups.get(code)
    if (bucket === undefined) groups.set(code, [entry])
    else bucket.push(entry)
  }

  return [...groups.entries()]
    .map(([code, group]) => ({ code, label: labelFor(code), entries: group }))
    .sort((a, b) => {
      if (a.code === MULTI) return -1
      if (b.code === MULTI) return 1
      return a.label.localeCompare(b.label)
    })
}

/**
 * A language code as a reader's own language names it.
 *
 * An index carries whatever a source author wrote, and `Intl` answers an
 * unrecognised tag by canonicalising it rather than by failing — `All` comes
 * back as `all`. A result that is only the code again means there is no name
 * to show, so the code is returned as it was written.
 */
export function languageName(code: string, locale: string): string {
  try {
    const named = new Intl.DisplayNames([locale], { type: 'language' }).of(code)
    if (named === undefined) return code
    return named.toLocaleLowerCase() === code.toLocaleLowerCase() ? code : named
  } catch {
    return code
  }
}

/**
 * The badge a rating earns, or nothing.
 *
 * Only ratings that warn: `safe` and `unknown` would be a badge on almost
 * every row, which makes the two that matter invisible.
 */
export function ratingBadge(rating: string): string | undefined {
  if (rating === 'suggestive') return '17+'
  if (rating === 'nsfw') return '18+'
  return undefined
}
