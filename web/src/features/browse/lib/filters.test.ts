import { describe, expect, it } from 'vitest'
import { type FilterState, parseFilters, parseState, toValues } from './filters'

/** The real declaration from `en.asurascans.aix`, shortened. */
const ASURA = [
  { type: 'sort', id: 'sort', canAscend: true, options: ['Latest Update', 'Popular'] },
  {
    type: 'select',
    id: 'status',
    title: 'Status',
    options: ['All', 'Ongoing', 'Completed'],
    ids: ['', 'ongoing', 'completed'],
  },
  { type: 'multi-select', id: 'genres', title: 'Genres', options: ['Action', 'Comedy'] },
]

describe('parseFilters', () => {
  it('reads a real package declaration', () => {
    const filters = parseFilters(ASURA)

    expect(filters.map((f) => f.kind)).toEqual(['sort', 'select', 'multi-select'])
    expect(filters[0]).toMatchObject({ id: 'sort', canAscend: true })
  })

  /**
   * `options` are labels and `ids` are what the source matches on. Sending the
   * label would filter on something it does not recognise.
   */
  it('keeps option labels and their sent values apart', () => {
    const status = parseFilters(ASURA)[1]
    expect(status).toMatchObject({
      options: ['All', 'Ongoing', 'Completed'],
      values: ['', 'ongoing', 'completed'],
    })
  })

  /** A mismatched `ids` array would pair a label with the wrong value. */
  it('ignores an ids array that does not line up', () => {
    const [filter] = parseFilters([
      { type: 'select', id: 's', options: ['a', 'b', 'c'], ids: ['x'] },
    ])
    expect(filter).toMatchObject({ values: ['a', 'b', 'c'] })
  })

  /**
   * `range` has no `FilterValue` variant on the host, so a control for it
   * would send something the server refuses. Named rather than hidden.
   */
  it('names a type the host has no value for', () => {
    const [filter] = parseFilters([{ type: 'range', id: 'year', title: 'Year' }])
    expect(filter).toMatchObject({ kind: 'unsupported', declaredType: 'range' })
  })

  /** Without an id there is nothing to address the value by. */
  it('drops an entry with no id', () => {
    expect(parseFilters([{ type: 'select', title: 'No id' }])).toEqual([])
  })

  /** Third-party data. Nothing here may throw on a shape it did not expect. */
  it('survives a declaration that is not what it should be', () => {
    expect(parseFilters(null)).toEqual([])
    expect(parseFilters('nonsense')).toEqual([])
    expect(parseFilters([null, 42, 'x', {}])).toEqual([])
  })
})

describe('toValues', () => {
  const filters = parseFilters(ASURA)

  it('orders by the declaration, not by when each was set', () => {
    const state: FilterState = {
      genres: { MultiSelect: { id: 'genres', included: ['Action'], excluded: [] } },
      status: { Select: { id: 'status', value: 'ongoing' } },
    }

    expect(toValues(filters, state).map((v) => Object.keys(v)[0])).toEqual([
      'Select',
      'MultiSelect',
    ])
  })

  /**
   * A multi-select with nothing chosen narrows to "anything", which some
   * sources read as "nothing". Dropped rather than sent empty.
   */
  it('drops a filter that is set to nothing', () => {
    const state: FilterState = {
      genres: { MultiSelect: { id: 'genres', included: [], excluded: [] } },
      status: { Select: { id: 'status', value: '' } },
    }
    expect(toValues(filters, state)).toEqual([])
  })

  it('keeps a sort, which is meaningful at index zero', () => {
    const state: FilterState = { sort: { Sort: { id: 'sort', index: 0, ascending: false } } }
    expect(toValues(filters, state)).toHaveLength(1)
  })
})

describe('parseState', () => {
  it('round-trips what toValues produced', () => {
    const filters = parseFilters(ASURA)
    const state: FilterState = { status: { Select: { id: 'status', value: 'ongoing' } } }

    expect(parseState(JSON.stringify(toValues(filters, state)))).toEqual(state)
  })

  /** A hand-edited URL is not a crash. */
  it('ignores anything malformed', () => {
    expect(parseState('not json')).toEqual({})
    expect(parseState('{"not":"an array"}')).toEqual({})
    expect(parseState('[1, null, "x", {}]')).toEqual({})
    expect(parseState(undefined)).toEqual({})
  })
})
