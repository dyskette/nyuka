import { describe, expect, it } from 'vitest'
import { decode, encode, fromBase64, PostcardError, toBase64, type ValueType } from './postcard'

/**
 * Printed by `crates/aidoku-runtime/tests/postcard_vectors.rs`.
 *
 * This codec is hand-written against a format description; these are what hold
 * it to the serializer that actually produces the bytes. Regenerate with:
 *
 *     cargo test -p nyuka-aidoku-runtime --test postcard_vectors -- --nocapture
 */
const VECTORS: [string, ValueType, unknown, number[]][] = [
  ['bool true', 'bool', true, [1]],
  ['bool false', 'bool', false, [0]],
  ['empty string', 'string', '', [0]],
  ['short string', 'string', 'on', [2, 111, 110]],
  ['word', 'string', 'ongoing', [7, 111, 110, 103, 111, 105, 110, 103]],
  // Three characters, nine bytes: the length prefix counts bytes.
  ['multi-byte', 'string', '日本語', [9, 230, 151, 165, 230, 156, 172, 232, 170, 158]],
  ['empty list', 'string-list', [], [0]],
  ['list', 'string-list', ['a', 'bb'], [2, 1, 97, 2, 98, 98]],
  ['zero', 'int', 0, [0]],
  // Zigzag, which is the part a reading of the format most easily gets wrong.
  ['one', 'int', 1, [2]],
  ['minus one', 'int', -1, [1]],
  ['past one byte', 'int', 300, [216, 4]],
]

describe('the vectors from the real serializer', () => {
  for (const [name, type, value, bytes] of VECTORS) {
    it(`decodes ${name}`, () => {
      expect(decode(Uint8Array.from(bytes), type)).toEqual(value)
    })

    it(`encodes ${name}`, () => {
      expect([...encode(value as never, type)]).toEqual(bytes)
    })
  }
})

/** A 200-byte string, whose length prefix needs two varint bytes. */
describe('a length that does not fit in one byte', () => {
  const long = 'x'.repeat(200)

  it('encodes the prefix as a two-byte varint', () => {
    const bytes = encode(long, 'string')
    expect([...bytes.subarray(0, 2)]).toEqual([200, 1])
    expect(bytes.length).toBe(202)
  })

  it('round-trips', () => {
    expect(decode(encode(long, 'string'), 'string')).toBe(long)
  })
})

describe('decoding refuses what it cannot be sure of', () => {
  /**
   * Postcard is not self-describing: nothing in `[1]` says whether it is
   * `true`, the integer 1, or a one-byte string. Leftover bytes are the one
   * signal that the declared type was wrong, and ignoring them would write
   * the misreading back on the next save.
   */
  it('rejects a value with bytes left over', () => {
    expect(() => decode(Uint8Array.from([1, 99]), 'bool')).toThrow(PostcardError)
    expect(() => decode(Uint8Array.from([2, 111, 110, 99]), 'string')).toThrow(PostcardError)
  })

  it('rejects a byte that is not a bool', () => {
    expect(() => decode(Uint8Array.from([2]), 'bool')).toThrow(/not a bool/)
  })

  /** A length prefix longer than the value left behind. */
  it('rejects a value shorter than its own length says', () => {
    expect(() => decode(Uint8Array.from([9, 111, 110]), 'string')).toThrow(PostcardError)
  })

  it('rejects an empty value', () => {
    expect(() => decode(new Uint8Array(), 'bool')).toThrow(PostcardError)
    expect(() => decode(new Uint8Array(), 'string')).toThrow(PostcardError)
  })

  /**
   * All-continuation bytes read to the end of the buffer and, unbounded, past
   * what a `number` holds — so a string's length prefix would come back as
   * something like 1e21 and the read below it would be whatever that
   * truncates to.
   */
  it('rejects a varint longer than a u32', () => {
    expect(() => decode(Uint8Array.from([0x80, 0x80, 0x80, 0x80, 0x80, 0x80]), 'int')).toThrow(
      /longer than a u32/,
    )
  })
})

describe('encoding refuses a value of the wrong shape', () => {
  it('will not encode a string as a bool, or the reverse', () => {
    expect(() => encode('yes' as never, 'bool')).toThrow(PostcardError)
    expect(() => encode(true as never, 'string')).toThrow(PostcardError)
  })

  it('will not encode a fraction as an integer', () => {
    expect(() => encode(1.5, 'int')).toThrow(PostcardError)
  })
})

describe('base64, which is how the API carries the bytes', () => {
  it('round-trips', () => {
    const bytes = Uint8Array.from([0, 1, 2, 253, 254, 255])
    expect([...fromBase64(toBase64(bytes))]).toEqual([...bytes])
  })

  /**
   * `String.fromCharCode(...value)` spreads every byte as an argument and
   * overflows the call stack somewhere around 100k. A setting value is capped
   * well below that by the server, but the failure would be a crash rather
   * than an error.
   */
  it('handles a value too large to spread', () => {
    const big = new Uint8Array(200_000).fill(65)
    expect(fromBase64(toBase64(big)).length).toBe(big.length)
  })
})
