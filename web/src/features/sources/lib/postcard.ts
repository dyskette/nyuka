/**
 * The postcard encoding, for the primitive values a source's settings hold.
 *
 * A source reads and writes its settings through the `defaults` host import,
 * which postcard-encodes them, and the API passes the bytes through
 * base64-encoded rather than guessing at a schema it does not have. The
 * *client* does have one: `GET /sources/{id}/settings` returns the package's
 * `settings.json` beside the values, and that declares each key's type.
 *
 * So this decodes by declared type, never by inspecting the bytes. Postcard is
 * a non-self-describing format — nothing in `[1]` says whether it is `true`,
 * the integer 1, or a one-byte string — so a decoder that guessed would be
 * guessing, and it would be right often enough to look correct.
 *
 * The encodings are pinned against the real serializer by
 * `crates/aidoku-runtime/tests/postcard_vectors.rs`, which prints them.
 *
 * Only the shapes the setting types need are implemented: bool, string,
 * string list, and signed integer. Anything else throws rather than returning
 * a value that might be wrong.
 */

export class PostcardError extends Error {}

// --- reading ---------------------------------------------------------------

interface Reader {
  bytes: Uint8Array
  at: number
}

/**
 * An LEB128 unsigned varint.
 *
 * Capped at five bytes, which covers a `u32`. Without a cap a crafted value of
 * all continuation bytes reads to the end of the buffer and, with a longer
 * one, past what a `number` can hold — so the length prefix of a string would
 * come back as something like 1e21 and the allocation below it would be
 * whatever that truncates to.
 */
function varint(reader: Reader): number {
  let result = 0
  let shift = 0

  for (let i = 0; i < 5; i += 1) {
    const byte = reader.bytes[reader.at]
    if (byte === undefined) throw new PostcardError('the value ended mid-number')
    reader.at += 1

    result += (byte & 0x7f) * 2 ** shift
    if ((byte & 0x80) === 0) return result
    shift += 7
  }

  throw new PostcardError('the number is longer than a u32')
}

/** Undoes zigzag, which is how postcard encodes a signed integer. */
function unzigzag(value: number): number {
  return value % 2 === 0 ? value / 2 : -(value + 1) / 2
}

function bytes(reader: Reader, count: number): Uint8Array {
  if (reader.at + count > reader.bytes.length) {
    throw new PostcardError('the value is shorter than its own length says')
  }
  const slice = reader.bytes.subarray(reader.at, reader.at + count)
  reader.at += count
  return slice
}

function readString(reader: Reader): string {
  return new TextDecoder().decode(bytes(reader, varint(reader)))
}

/** The types a setting's declaration can name. */
export type ValueType = 'bool' | 'string' | 'string-list' | 'int'

export type Value = boolean | string | string[] | number

/**
 * Decodes one value, given the type its declaration says it has.
 *
 * Trailing bytes are an error rather than ignored: a value that decoded
 * successfully and left bytes behind was not the type it was read as, and
 * accepting it would write that misreading back on the next save.
 */
export function decode(raw: Uint8Array, type: ValueType): Value {
  const reader: Reader = { bytes: raw, at: 0 }
  let value: Value

  switch (type) {
    case 'bool': {
      const byte = bytes(reader, 1)[0]
      if (byte !== 0 && byte !== 1) throw new PostcardError(`${byte} is not a bool`)
      value = byte === 1
      break
    }
    case 'string':
      value = readString(reader)
      break
    case 'string-list': {
      const count = varint(reader)
      const list: string[] = []
      for (let i = 0; i < count; i += 1) list.push(readString(reader))
      value = list
      break
    }
    case 'int':
      value = unzigzag(varint(reader))
      break
  }

  if (reader.at !== raw.length) {
    throw new PostcardError(`${raw.length - reader.at} bytes left after a ${type}`)
  }
  return value
}

// --- writing ---------------------------------------------------------------

function writeVarint(out: number[], value: number): void {
  if (!Number.isInteger(value) || value < 0) {
    throw new PostcardError(`${value} is not a length or an unsigned integer`)
  }
  let rest = value
  while (rest >= 0x80) {
    out.push((rest & 0x7f) | 0x80)
    rest = Math.floor(rest / 128)
  }
  out.push(rest)
}

function writeString(out: number[], value: string): void {
  const encoded = new TextEncoder().encode(value)
  // The length is in bytes, not characters — "日本語" is three characters and
  // nine bytes, and a source reading a character count would read past its
  // own value into the next one.
  writeVarint(out, encoded.length)
  out.push(...encoded)
}

export function encode(value: Value, type: ValueType): Uint8Array {
  const out: number[] = []

  switch (type) {
    case 'bool':
      if (typeof value !== 'boolean') throw new PostcardError('expected a bool')
      out.push(value ? 1 : 0)
      break
    case 'string':
      if (typeof value !== 'string') throw new PostcardError('expected a string')
      writeString(out, value)
      break
    case 'string-list':
      if (!Array.isArray(value)) throw new PostcardError('expected a list')
      writeVarint(out, value.length)
      for (const item of value) writeString(out, item)
      break
    case 'int': {
      if (typeof value !== 'number' || !Number.isInteger(value)) {
        throw new PostcardError('expected an integer')
      }
      // Zigzag: postcard maps a signed integer onto an unsigned one so small
      // negatives stay short. -1 encodes as 1, not as five bytes of sign
      // extension.
      writeVarint(out, value < 0 ? -value * 2 - 1 : value * 2)
      break
    }
  }

  return Uint8Array.from(out)
}

// --- base64, which is how the API carries the bytes ------------------------

export function fromBase64(value: string): Uint8Array {
  const binary = atob(value)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export function toBase64(value: Uint8Array): string {
  // Built in one pass rather than through `String.fromCharCode(...value)`,
  // which spreads every byte as an argument and overflows the call stack on a
  // large value.
  let binary = ''
  for (const byte of value) binary += String.fromCharCode(byte)
  return btoa(binary)
}
