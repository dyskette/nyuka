/**
 * Byte and rate formatting.
 *
 * Binary units (KiB, MiB) rather than decimal: the sizes shown here come from
 * file sizes on disk, which every file manager reports in binary multiples.
 * Using 1000 would make this disagree with `ls -lh` about the same file.
 */
const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'

  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }

  // Whole bytes never show a decimal: "512.0 B" is noise.
  const digits = unit === 0 ? 0 : value < 10 ? 1 : 0
  return `${value.toFixed(digits)} ${UNITS[unit]}`
}

/** A transfer rate, from bytes per second. */
export function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond < 0) return '—'
  return `${formatBytes(bytesPerSecond)}/s`
}
