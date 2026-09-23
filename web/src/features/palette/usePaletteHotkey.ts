import { useEffect } from 'react'

/**
 * Binds ⌘K on Apple keyboards and Ctrl+K elsewhere.
 *
 * `metaKey || ctrlKey` rather than sniffing the platform: a user on a Mac with
 * an external PC keyboard presses Ctrl, and a platform check gets them wrong.
 * Accepting both costs nothing — no browser binds the other one to anything
 * this would shadow.
 *
 * `event.key` rather than `event.code`: `KeyK` is the physical key, which on a
 * Dvorak or AZERTY layout is not the one labelled K.
 */
export function usePaletteHotkey(onToggle: () => void): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key.toLowerCase() !== 'k') return
      if (!event.metaKey && !event.ctrlKey) return
      // Ctrl+Shift+K is the browser's own console in Firefox; not ours.
      if (event.shiftKey || event.altKey) return

      // Firefox binds Ctrl+K to the search bar and Safari to the sidebar, so
      // this has to be prevented or the palette opens behind a focused
      // browser control.
      event.preventDefault()
      onToggle()
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onToggle])
}
