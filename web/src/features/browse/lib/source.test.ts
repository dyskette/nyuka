import { describe, expect, it } from 'vitest'
import { activeSource } from './source'

describe('activeSource', () => {
  const installed = [{ id: 'first' }, { id: 'second' }]

  it('uses the source the URL names', () => {
    expect(activeSource('second', installed)).toBe('second')
  })

  /**
   * The case the detail panel got wrong: arriving from the sidebar, nothing
   * is in the URL and the grid is already showing the first source.
   */
  it('falls back to the first installed source', () => {
    expect(activeSource(undefined, installed)).toBe('first')
  })

  /**
   * Honoured even when it is not installed, so the screen can say the source
   * is gone rather than quietly showing a different one's catalog under the
   * same address.
   */
  it('does not substitute for a source that is named but missing', () => {
    expect(activeSource('uninstalled', installed)).toBe('uninstalled')
  })

  it('has no answer when nothing is installed', () => {
    expect(activeSource(undefined, [])).toBeUndefined()
  })
})
