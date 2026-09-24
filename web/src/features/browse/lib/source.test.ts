import { describe, expect, it } from 'vitest'
import { activeSource } from './source'

describe('activeSource', () => {
  const installed = [{ id: 'first' }, { id: 'second' }]

  it('uses the source the URL names', () => {
    expect(activeSource('second', installed)).toBe('second')
  })

  /** Arriving from the sidebar, with nothing in the URL. */
  it('falls back to the first installed source', () => {
    expect(activeSource(undefined, installed)).toBe('first')
  })

  /**
   * Honoured even when missing, so the screen can say the source is gone
   * rather than show another one's catalog under the same address.
   */
  it('does not substitute for a source that is named but missing', () => {
    expect(activeSource('uninstalled', installed)).toBe('uninstalled')
  })

  it('has no answer when nothing is installed', () => {
    expect(activeSource(undefined, [])).toBeUndefined()
  })
})
