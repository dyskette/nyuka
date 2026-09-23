/**
 * A source's filter declaration, as its package's `filters.json` states it.
 *
 * The server passes this through without interpreting it, so everything here
 * is third-party data: every field is checked, and an entry that does not fit
 * is skipped rather than coerced.
 *
 * Surveyed across the 121 of 136 community sources that declare filters:
 *
 * | type           | count | here |
 * |----------------|-------|------|
 * | `select`       |   221 | yes  |
 * | `multi-select` |   150 | yes  |
 * | `sort`         |    98 | yes  |
 * | `text`         |    45 | yes  |
 * | `range`        |    18 | no   |
 * | `check`        |     2 | yes  |
 *
 * `range` is left out because the *host* has no matching `FilterValue`
 * variant. The gap is there, not here, and rendering a control whose value the
 * server would refuse is worse than leaving it out.
 */

/** What the server accepts, mirroring the host's `FilterValue`. */
export type FilterValue =
  | { Text: { id: string; value: string } }
  | { Sort: { id: string; index: number; ascending: boolean } }
  | { Check: { id: string; value: number } }
  | { Select: { id: string; value: string } }
  | { MultiSelect: { id: string; included: string[]; excluded: string[] } }

interface Common {
  id: string
  title: string
}

export type Filter =
  | (Common & { kind: 'select'; options: string[]; values: string[] })
  | (Common & { kind: 'multi-select'; options: string[]; values: string[]; canExclude: boolean })
  | (Common & { kind: 'sort'; options: string[]; canAscend: boolean })
  | (Common & { kind: 'text' })
  | (Common & { kind: 'check' })
  | (Common & { kind: 'unsupported'; declaredType: string })

function asString(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

function asStrings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : []
}

/**
 * Reads a source's declaration.
 *
 * An entry with no `id` is dropped rather than reported: every filter value is
 * addressed by one, so there is nothing a reader could do with it. An entry
 * with a *type* this build does not handle is named instead, because that is a
 * gap worth seeing.
 */
export function parseFilters(declared: unknown): Filter[] {
  if (!Array.isArray(declared)) return []

  return declared.flatMap((node): Filter[] => {
    if (typeof node !== 'object' || node === null) return []
    const entry = node as Record<string, unknown>

    const id = asString(entry.id)
    if (id === null) return []

    const declaredType = asString(entry.type) ?? ''
    const title = asString(entry.title) ?? id
    const options = asStrings(entry.options)
    // `ids` is what a source matches on; `options` is what it shows. They
    // differ whenever a label is prettier than the value behind it, and
    // sending the label would filter on something it does not recognise.
    const ids = asStrings(entry.ids)
    const values = ids.length === options.length ? ids : options

    switch (declaredType) {
      case 'select':
        return [{ kind: 'select', id, title, options, values }]
      case 'multi-select':
        return [
          {
            kind: 'multi-select',
            id,
            title,
            options,
            values,
            canExclude: entry.canExclude === true,
          },
        ]
      case 'sort':
        return [{ kind: 'sort', id, title, options, canAscend: entry.canAscend === true }]
      case 'text':
        return [{ kind: 'text', id, title }]
      case 'check':
        return [{ kind: 'check', id, title }]
      default:
        return declaredType === '' ? [] : [{ kind: 'unsupported', id, title, declaredType }]
    }
  })
}

/** The value currently set for each filter, by filter id. */
export type FilterState = Record<string, FilterValue>

/**
 * The values to send, in declaration order.
 *
 * Ordered by the declaration rather than by when each was set, so the same
 * selection always produces the same URL — otherwise going back and forward
 * would rewrite it for no reason.
 *
 * A filter set to nothing is dropped rather than sent empty: a multi-select
 * with no options chosen narrows to "anything", which some sources read as
 * "nothing".
 */
export function toValues(filters: Filter[], state: FilterState): FilterValue[] {
  return filters
    .map((filter) => state[filter.id])
    .filter((value): value is FilterValue => {
      if (value === undefined) return false
      if ('MultiSelect' in value) {
        return value.MultiSelect.included.length > 0 || value.MultiSelect.excluded.length > 0
      }
      if ('Text' in value) return value.Text.value !== ''
      if ('Select' in value) return value.Select.value !== ''
      return true
    })
}

/** Reads the state back out of a URL parameter, ignoring anything malformed. */
export function parseState(raw: string | undefined): FilterState {
  if (raw === undefined || raw === '') return {}
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return {}

    const state: FilterState = {}
    for (const value of parsed) {
      const id = filterId(value)
      if (id !== null) state[id] = value as FilterValue
    }
    return state
  } catch {
    // A hand-edited URL is not a crash.
    return {}
  }
}

function filterId(value: unknown): string | null {
  if (typeof value !== 'object' || value === null) return null
  const inner = Object.values(value as Record<string, unknown>)[0]
  if (typeof inner !== 'object' || inner === null) return null
  return asString((inner as Record<string, unknown>).id)
}
