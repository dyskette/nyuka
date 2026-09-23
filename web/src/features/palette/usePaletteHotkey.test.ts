import { renderHook } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { usePaletteHotkey } from './usePaletteHotkey'

function press(init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { cancelable: true, ...init })
  window.dispatchEvent(event)
  return event
}

describe('usePaletteHotkey', () => {
  /**
   * Both modifiers, rather than a platform check: a Mac with an external PC
   * keyboard presses Ctrl, and sniffing the platform gets that user wrong.
   */
  it('fires on ⌘K and on Ctrl+K', () => {
    const onToggle = vi.fn()
    renderHook(() => usePaletteHotkey(onToggle))

    press({ key: 'k', metaKey: true })
    press({ key: 'k', ctrlKey: true })

    expect(onToggle).toHaveBeenCalledTimes(2)
  })

  it('accepts the shifted capital the key reports', () => {
    const onToggle = vi.fn()
    renderHook(() => usePaletteHotkey(onToggle))

    press({ key: 'K', metaKey: true })
    expect(onToggle).toHaveBeenCalledTimes(1)
  })

  /**
   * Firefox binds Ctrl+K to its search bar and Safari to the sidebar. Without
   * preventing the default the palette opens behind a focused browser
   * control, which reads as the palette not taking keyboard input.
   */
  it('prevents the browser taking the chord', () => {
    renderHook(() => usePaletteHotkey(vi.fn()))

    expect(press({ key: 'k', metaKey: true }).defaultPrevented).toBe(true)
  })

  it('ignores K with no modifier, so typing a k is just a k', () => {
    const onToggle = vi.fn()
    renderHook(() => usePaletteHotkey(onToggle))

    press({ key: 'k' })
    expect(onToggle).not.toHaveBeenCalled()
  })

  /** Ctrl+Shift+K is Firefox's console. Not ours to take. */
  it('ignores the chord with extra modifiers', () => {
    const onToggle = vi.fn()
    renderHook(() => usePaletteHotkey(onToggle))

    press({ key: 'k', ctrlKey: true, shiftKey: true })
    press({ key: 'k', metaKey: true, altKey: true })

    expect(onToggle).not.toHaveBeenCalled()
  })

  it('ignores other keys', () => {
    const onToggle = vi.fn()
    renderHook(() => usePaletteHotkey(onToggle))

    press({ key: 'j', metaKey: true })
    expect(onToggle).not.toHaveBeenCalled()
  })

  /** A listener outliving its component keeps toggling a palette that is gone. */
  it('detaches on unmount', () => {
    const onToggle = vi.fn()
    const { unmount } = renderHook(() => usePaletteHotkey(onToggle))

    unmount()
    press({ key: 'k', metaKey: true })

    expect(onToggle).not.toHaveBeenCalled()
  })
})
