import { useQueryClient } from '@tanstack/react-query'
import { createContext, type ReactNode, use, useEffect, useState } from 'react'
import { EVENT_NAMES, type EventName, LAGGED_EVENT, parseEvent } from './events'
import { applyEvent, invalidateEverything } from './invalidate'

export type LiveStatus = 'connecting' | 'live' | 'offline'

const LiveContext = createContext<LiveStatus>('connecting')

/** The stream's current state, for the status bar. */
export function useLiveStatus(): LiveStatus {
  return use(LiveContext)
}

/**
 * One `EventSource` for the whole application (ADR-0010).
 *
 * Mounted once at the root rather than per screen: an SSE stream occupies one
 * of the browser's six HTTP/1.1 connections per origin, and a second one for
 * a second screen leaves four for everything else — after which the seventh
 * request of any kind queues with no error anywhere.
 *
 * # Reconnection is where correctness lives
 *
 * `EventSource` reconnects on its own, using the server's `retry:`. What it
 * cannot do is tell the application what happened while it was gone, and no
 * replay exists to ask for (the server emits no `id:`, deliberately). So
 * every successful (re)connection invalidates the cache wholesale. A `lagged`
 * event means the same thing — the server saying it dropped events for this
 * client — and is handled identically.
 *
 * A first connection therefore also invalidates, which is one redundant
 * refetch of data a route loader just fetched. That is the cost of having one
 * rule instead of a "was this the first time" flag that is wrong exactly when
 * a reconnect happens during startup.
 */
export function LiveProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()
  const [status, setStatus] = useState<LiveStatus>('connecting')

  useEffect(() => {
    // Same origin, so the cookie goes automatically — which is the whole
    // reason for the BFF pattern: `EventSource` cannot send headers
    // (ADR-0005, ADR-0010).
    const source = new EventSource('/api/v1/events')

    source.onopen = () => {
      setStatus('live')
      invalidateEverything(queryClient)
    }

    // Fired on every drop as well as on a failure to connect. `EventSource`
    // retries by itself, so this reports rather than reconnects — calling
    // `close()` here would turn a transient blip into a permanently dead
    // stream.
    source.onerror = () => setStatus('offline')

    const listeners = EVENT_NAMES.map((name: EventName) => {
      const listener = (event: MessageEvent<string>) => {
        const payload = parseEvent(event.data)
        if (payload !== null) applyEvent(queryClient, name, payload)
      }
      source.addEventListener(name, listener)
      return [name, listener] as const
    })

    const onLagged = () => invalidateEverything(queryClient)
    source.addEventListener(LAGGED_EVENT, onLagged)

    return () => {
      for (const [name, listener] of listeners) source.removeEventListener(name, listener)
      source.removeEventListener(LAGGED_EVENT, onLagged)
      source.close()
    }
  }, [queryClient])

  return <LiveContext value={status}>{children}</LiveContext>
}
