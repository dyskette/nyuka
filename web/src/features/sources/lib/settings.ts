import { decode, fromBase64, type Value, type ValueType } from './postcard'

/**
 * A source's settings declaration, as its package's `settings.json` states it.
 *
 * The server passes this through verbatim and does not interpret it, so
 * everything here is third-party data: every field is optional, every type is
 * checked, and an entry that does not fit is skipped rather than coerced.
 *
 * The shape is Aidoku's convention, surveyed across 136 community sources:
 *
 * | type            | count | here |
 * |-----------------|-------|------|
 * | `group`         |    87 | container |
 * | `select`        |    31 | yes  |
 * | `switch`        |    29 | yes  |
 * | `login`         |    26 | no — needs a webview (ADR-0004 declared gap) |
 * | `text`          |    20 | yes  |
 * | `multi-select`  |    11 | yes  |
 * | `editable-list` |     6 | no   |
 * | `button`        |     5 | no — an action, not a value |
 * | `segment`       |     4 | yes  |
 * | `link`          |     2 | no — an action |
 * | `page`          |     2 | no — nesting |
 *
 * The five handled types cover 95 of the 136 controls. The rest are listed by
 * name in the UI rather than hidden, so a reader can see that a source has a
 * setting this build cannot offer.
 */

/** A control this build can read and write. */
export interface EditableSetting {
  kind: 'switch' | 'text' | 'select' | 'segment' | 'multi-select'
  key: string
  title: string
  /** Option labels, for the three that have them. */
  options: string[]
  /** What each option writes, when it differs from its label. */
  values: string[]
  valueType: ValueType
}

/** A control this build knows about and cannot offer. */
export interface UnsupportedSetting {
  kind: 'unsupported'
  declaredType: string
  title: string
}

export type Setting = EditableSetting | UnsupportedSetting

export interface SettingGroup {
  title: string | null
  settings: Setting[]
}

const VALUE_TYPES: Record<EditableSetting['kind'], ValueType> = {
  switch: 'bool',
  text: 'string',
  select: 'string',
  segment: 'string',
  'multi-select': 'string-list',
}

function asString(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

function asStrings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : []
}

/**
 * Reads one declared entry.
 *
 * Returns `null` for anything without a key, since a setting with no key
 * cannot be written back — the `PUT` addresses it by key.
 */
function toSetting(node: unknown): Setting | null {
  if (typeof node !== 'object' || node === null) return null
  const entry = node as Record<string, unknown>

  const declaredType = asString(entry.type) ?? ''
  const title = asString(entry.title) ?? asString(entry.key) ?? declaredType

  if (!(declaredType in VALUE_TYPES)) {
    return declaredType === '' ? null : { kind: 'unsupported', declaredType, title }
  }

  const key = asString(entry.key)
  if (key === null) return { kind: 'unsupported', declaredType, title }

  const kind = declaredType as EditableSetting['kind']
  const options = asStrings(entry.options)
  // `values` is what a source stores; `options` is what it shows. They differ
  // whenever a label is prettier than the value behind it, and writing the
  // label would store something the source does not recognise.
  const values = asStrings(entry.values)

  return {
    kind,
    key,
    title,
    options,
    values: values.length === options.length ? values : options,
    valueType: VALUE_TYPES[kind],
  }
}

/**
 * Flattens a declaration into groups.
 *
 * One level of nesting only: `group` holds `items`, and a `page` holding more
 * groups is reported as unsupported rather than flattened, because flattening
 * it would present a source's second screen as though it were its first.
 */
export function parseDeclaration(declared: unknown): SettingGroup[] {
  if (!Array.isArray(declared)) return []

  const groups: SettingGroup[] = []
  let loose: Setting[] = []

  for (const node of declared) {
    if (typeof node !== 'object' || node === null) continue
    const entry = node as Record<string, unknown>

    if (entry.type === 'group') {
      const settings = asArray(entry.items).map(toSetting).filter(isSetting)
      if (settings.length > 0) groups.push({ title: asString(entry.title), settings })
      continue
    }

    const setting = toSetting(entry)
    if (setting !== null) loose.push(setting)
  }

  if (loose.length > 0) {
    groups.push({ title: null, settings: loose })
    loose = []
  }
  return groups
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : []
}

function isSetting(value: Setting | null): value is Setting {
  return value !== null
}

/**
 * Decodes the stored values a source has, by the type its declaration gives.
 *
 * A key that fails to decode is left out rather than guessed at: the control
 * then shows its default, which is what the source itself falls back to. A
 * wrong value shown as current is the one outcome that would have the reader
 * save it back.
 */
export function decodeValues(
  settings: Setting[],
  // `null` as well as absent: the server omits an unset value, but the schema
  // permits null and a client that only handled `undefined` would try to
  // base64-decode it.
  stored: { key: string; value?: string | null | undefined }[],
): Map<string, Value> {
  const byKey = new Map(stored.map((entry) => [entry.key, entry.value]))
  const decoded = new Map<string, Value>()

  for (const setting of settings) {
    if (setting.kind === 'unsupported') continue
    const raw = byKey.get(setting.key)
    if (raw === undefined || raw === null) continue
    try {
      decoded.set(setting.key, decode(fromBase64(raw), setting.valueType))
    } catch {
      // Left unset on purpose. See the note above.
    }
  }
  return decoded
}
