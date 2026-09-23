import { describe, expect, it } from 'vitest'
import { formatBytes, formatRate } from './bytes'

describe('formatBytes', () => {
  /**
   * Binary units, so this agrees with `ls -lh` about the same file. Decimal
   * would report a 1 048 576-byte file as 1.0 MB against the file manager's
   * 1.0 MiB, and the difference grows with size.
   */
  it('uses binary multiples', () => {
    expect(formatBytes(1024)).toBe('1.0 KiB')
    expect(formatBytes(1024 * 1024)).toBe('1.0 MiB')
    expect(formatBytes(1536)).toBe('1.5 KiB')
  })

  it('shows no decimal for whole bytes', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(512)).toBe('512 B')
  })

  /** Precision drops once the number is wide enough to read without it. */
  it('drops the decimal past ten', () => {
    expect(formatBytes(34 * 1024 * 1024)).toBe('34 MiB')
  })

  it('stops at the largest unit it knows', () => {
    expect(formatBytes(1024 ** 5)).toBe('1024 TiB')
  })

  /**
   * A negative or non-finite size is a bug upstream. Rendering "NaN B" in a
   * table cell hides it; a dash reads as "not known", which is true.
   */
  it('refuses a value that is not a size', () => {
    expect(formatBytes(Number.NaN)).toBe('—')
    expect(formatBytes(-1)).toBe('—')
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('—')
  })
})

describe('formatRate', () => {
  it('reads as a rate', () => {
    expect(formatRate(1024 * 1024)).toBe('1.0 MiB/s')
  })

  it('refuses a value that is not a rate', () => {
    expect(formatRate(Number.NaN)).toBe('—')
  })
})
