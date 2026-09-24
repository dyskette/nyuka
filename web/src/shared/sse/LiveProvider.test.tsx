import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { libraryKeys } from '@/features/library/api/keys'
import { LiveProvider, useLiveStatus } from './LiveProvider'

/**
 * A stand-in for the browser's `EventSource`, which jsdom does not implement.
 *
 * Deliberately minimal: it records listeners and exposes a way to fire them,
 * so a test drives the connection lifecycle directly rather than waiting on a
 * real one.
 */
class FakeEventSource {
  static instances: FakeEventSource[] = []

  readonly url: string
  closed = false
  onopen: (() => void) | null = null
  onerror: (() => void) | null = null
  private readonly listeners = new Map<string, Set<(event: MessageEvent<string>) => void>>()

  constructor(url: string) {
    this.url = url
    FakeEventSource.instances.push(this)
  }

  addEventListener(name: string, listener: (event: MessageEvent<string>) => void) {
    const set = this.listeners.get(name) ?? new Set()
    set.add(listener)
    this.listeners.set(name, set)
  }

  removeEventListener(name: string, listener: (event: MessageEvent<string>) => void) {
    this.listeners.get(name)?.delete(listener)
  }

  close() {
    this.closed = true
  }

  /** Test-only: deliver an event as the server would. */
  emit(name: string, data: string) {
    for (const listener of this.listeners.get(name) ?? []) {
      listener({ data } as MessageEvent<string>)
    }
  }

  /** Test-only: how many listeners are attached, for the cleanup assertion. */
  get listenerCount(): number {
    return [...this.listeners.values()].reduce((total, set) => total + set.size, 0)
  }
}

function Status() {
  return <span data-testid="status">{useLiveStatus()}</span>
}

function setup() {
  const queryClient = new QueryClient()
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockReturnValue(Promise.resolve())

  const result = render(
    <QueryClientProvider client={queryClient}>
      <LiveProvider>
        <Status />
      </LiveProvider>
    </QueryClientProvider>,
  )

  const source = FakeEventSource.instances.at(-1)
  if (source === undefined) throw new Error('LiveProvider opened no EventSource')
  return { ...result, source, invalidate }
}

beforeEach(() => {
  FakeEventSource.instances = []
  vi.stubGlobal('EventSource', FakeEventSource)
})

afterEach(() => vi.unstubAllGlobals())

describe('LiveProvider', () => {
  it('opens one stream, same-origin so the cookie rides along', () => {
    const { source } = setup()

    expect(FakeEventSource.instances).toHaveLength(1)
    // Relative, not absolute: `EventSource` cannot send headers, so the whole
    // design depends on the cookie, which depends on one origin (ADR-0005).
    expect(source.url).toBe('/api/v1/events')
  })

  /**
   * This is ADR-0010's correctness claim. The client cannot know what changed
   * while it was disconnected and there is no replay to ask for, so every
   * connection assumes everything is stale.
   */
  it('invalidates the cache on every connection', () => {
    const { source, invalidate } = setup()

    expect(invalidate).not.toHaveBeenCalled()
    act(() => source.onopen?.())
    expect(invalidate).toHaveBeenCalledWith({ queryKey: libraryKeys.all })

    invalidate.mockClear()
    act(() => source.onerror?.())
    act(() => source.onopen?.())
    expect(invalidate).toHaveBeenCalledWith({ queryKey: libraryKeys.all })
  })

  /** A `lagged` event is the server saying it dropped events for this client. */
  it('invalidates the cache when the server says it fell behind', () => {
    const { source, invalidate } = setup()
    act(() => source.onopen?.())
    invalidate.mockClear()

    act(() => source.emit('lagged', '12'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: libraryKeys.all })
  })

  it('routes an event to the keys it makes stale', () => {
    const { source, invalidate } = setup()
    act(() => source.onopen?.())
    invalidate.mockClear()

    act(() =>
      source.emit(
        'chapter.downloaded',
        '{"event":"chapter_downloaded","manga_id":"m-1","chapter_id":"c-1"}',
      ),
    )

    expect(invalidate).toHaveBeenCalledWith({ queryKey: libraryKeys.detail('m-1') })
  })

  /**
   * `EventSource` reconnects by itself. Calling `close()` on an error would
   * turn a transient network blip into a permanently dead stream — and the
   * UI would look fine, because nothing else reports it.
   */
  it('reports a dropped connection without closing the stream', () => {
    const { source } = setup()
    act(() => source.onopen?.())

    act(() => source.onerror?.())

    expect(screen.getByTestId('status')).toHaveTextContent('offline')
    expect(source.closed).toBe(false)
  })

  it('shows the connection state it is in', () => {
    const { source } = setup()

    expect(screen.getByTestId('status')).toHaveTextContent('connecting')
    act(() => source.onopen?.())
    expect(screen.getByTestId('status')).toHaveTextContent('live')
  })

  /**
   * An unmounted provider that keeps its stream open leaks a connection
   * against a six-per-origin budget, and its listeners keep invalidating a
   * cache nothing is reading.
   */
  it('closes the stream and detaches its listeners on unmount', () => {
    const { source, unmount } = setup()
    expect(source.listenerCount).toBeGreaterThan(0)

    unmount()

    expect(source.closed).toBe(true)
    expect(source.listenerCount).toBe(0)
  })
})
