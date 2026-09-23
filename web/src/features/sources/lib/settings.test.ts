import { describe, expect, it } from 'vitest'
import { encode, toBase64 } from './postcard'
import { decodeValues, type EditableSetting, parseDeclaration } from './settings'

/** The real declaration from `en.asurascans.aix`, shortened. */
const ASURA = [
  {
    type: 'group',
    title: 'SETTINGS',
    items: [{ type: 'switch', key: 'showLocked', title: 'Show Locked Chapters', default: true }],
  },
  {
    type: 'group',
    items: [
      {
        type: 'login',
        key: 'login',
        method: 'web',
        title: 'LOGIN',
        url: 'https://asurascans.com/login',
      },
    ],
  },
]

describe('parseDeclaration', () => {
  it('reads a real package declaration', () => {
    const groups = parseDeclaration(ASURA)

    expect(groups.map((g) => g.title)).toEqual(['SETTINGS', null])
    expect(groups[0]?.settings[0]).toMatchObject({
      kind: 'switch',
      key: 'showLocked',
      title: 'Show Locked Chapters',
      valueType: 'bool',
    })
  })

  /**
   * A `login` needs a webview, which ADR-0004 lists as a declared capability
   * gap. It is reported rather than dropped, so a reader can see the source
   * has a setting this build cannot offer.
   */
  it('names what it cannot offer instead of hiding it', () => {
    const groups = parseDeclaration(ASURA)

    expect(groups[1]?.settings[0]).toEqual({
      kind: 'unsupported',
      declaredType: 'login',
      title: 'LOGIN',
    })
  })

  /**
   * `options` are labels and `values` are what gets stored. Writing the label
   * would store something the source does not recognise.
   */
  it('keeps option labels and their stored values apart', () => {
    const groups = parseDeclaration([
      {
        type: 'select',
        key: 'status',
        title: 'Status',
        options: ['Any', 'Ongoing'],
        values: ['', 'ongoing'],
      },
    ])

    expect(groups[0]?.settings[0]).toMatchObject({
      options: ['Any', 'Ongoing'],
      values: ['', 'ongoing'],
    })
  })

  /** A declaration that lists only labels stores the labels. */
  it('falls back to the labels when no values are given', () => {
    const groups = parseDeclaration([{ type: 'select', key: 's', title: 'S', options: ['a', 'b'] }])
    expect((groups[0]?.settings[0] as EditableSetting).values).toEqual(['a', 'b'])
  })

  /**
   * A mismatched `values` array would pair a label with the wrong value —
   * silently storing "ongoing" for "Completed". Falling back to the labels is
   * wrong in a visible way instead.
   */
  it('ignores a values array that does not line up with the options', () => {
    const groups = parseDeclaration([
      { type: 'select', key: 's', title: 'S', options: ['a', 'b', 'c'], values: ['x'] },
    ])
    expect((groups[0]?.settings[0] as EditableSetting).values).toEqual(['a', 'b', 'c'])
  })

  /** A setting with no key cannot be written back: `PUT` addresses it by key. */
  it('reports a keyless control as unsupported', () => {
    const groups = parseDeclaration([{ type: 'switch', title: 'No key' }])
    expect(groups[0]?.settings[0]).toMatchObject({ kind: 'unsupported' })
  })

  /** Third-party data. Nothing here may throw on a shape it did not expect. */
  it('survives a declaration that is not what it should be', () => {
    expect(parseDeclaration(null)).toEqual([])
    expect(parseDeclaration('nonsense')).toEqual([])
    expect(parseDeclaration([null, 42, 'x', {}])).toEqual([])
    expect(parseDeclaration([{ type: 'group' }])).toEqual([])
    expect(parseDeclaration([{ type: 'group', items: 'no' }])).toEqual([])
  })
})

describe('decodeValues', () => {
  const settings = parseDeclaration([
    { type: 'switch', key: 'locked', title: 'Locked' },
    { type: 'text', key: 'token', title: 'Token' },
  ])[0]!.settings

  it('decodes each value by its declared type', () => {
    const values = decodeValues(settings, [
      { key: 'locked', value: toBase64(encode(true, 'bool')) },
      { key: 'token', value: toBase64(encode('abc', 'string')) },
    ])

    expect(values.get('locked')).toBe(true)
    expect(values.get('token')).toBe('abc')
  })

  /**
   * A value that will not decode is left unset, so the control shows its
   * default — which is what the source itself falls back to. Showing a wrong
   * value as current is the one outcome that gets it saved back.
   */
  it('leaves a value it cannot decode unset', () => {
    const values = decodeValues(settings, [
      { key: 'locked', value: toBase64(encode('not a bool', 'string')) },
    ])

    expect(values.has('locked')).toBe(false)
  })

  it('ignores stored keys the declaration does not mention', () => {
    const values = decodeValues(settings, [
      { key: 'somethingElse', value: toBase64(encode(true, 'bool')) },
    ])

    expect(values.size).toBe(0)
  })

  it('leaves an absent value unset rather than guessing a default', () => {
    expect(decodeValues(settings, []).size).toBe(0)
  })
})
