# ADR-0010: Drive live state through one SSE stream into the Query cache

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/shared/api/sse.ts`, `crates/api` (SSE handler), `crates/jobs` |
| **Supersedes** | None |

This article explains why live updates arrive over a single Server-Sent Events stream whose handlers write only into the TanStack Query cache, and why no component ever touches the `EventSource`. It also records the two operational constraints that make SSE work in production — connection limits and proxy buffering — because both fail quietly when overlooked.

## Context

Downloads run for seconds to minutes. A user who queues a chapter expects to watch progress, see the state change, and get a new-chapter badge without refreshing. That is the only requirement for pushed data in the product; everything else is request/response.

### Decisions already made constrain the transport

- **[ADR-0003](0003-run-the-job-queue-in-postgres-and-in-process.md)** puts job workers in the API process publishing `JobEvent` to a `tokio::sync::broadcast` channel. A stream handler subscribing to that channel is the whole server side.
- **[ADR-0005](0005-act-as-the-oidc-client-with-server-side-sessions.md)** authenticates with a session cookie, in part *because* `EventSource` cannot set an `Authorization` header. A cookie-authenticated stream needs no special handling.
- **[ADR-0009](0009-hand-write-query-hooks-with-semantic-key-factories.md)** defines semantic key factories. Those keys are what an event handler addresses.
- **[ADR-0006](0006-embed-the-frontend-in-the-binary.md)** puts the SPA on the same origin, so there is no cross-origin `withCredentials` case.

### The cache is already the single source of server state

Per ADR-0008, components read server state exclusively through `useQuery` and `useInfiniteQuery`; route loaders only prime that cache. If pushed updates were delivered anywhere else — component state, a context, a store — the application would hold the same resource in two places, and they would disagree.

## Decision

Open **one** `EventSource` for the whole application and have its handlers write **only** into the Query cache.

- **One connection, one owner.** `shared/api/sse.ts` opens `EventSource('/api/v1/events')` from a provider mounted after authentication succeeds. No component constructs an `EventSource`, and no component subscribes to it.
- **Handlers address the cache through key factories**, never with inline key literals, per the invariant in ADR-0009.

  | Event | Cache action |
  |---|---|
  | `job.progress` | `setQueryData(jobsKeys.detail(id))`; patch that row in the infinite list in place |
  | `job.state` | Patch, invalidate `jobsKeys.lists()`, toast on `failed` |
  | `chapter.new` | Invalidate `libraryKeys.chapters(mangaId)` and the follows badge |
  | `chapter.downloaded` | Patch the chapter row, invalidate storage stats |
  | `source.updated` | Invalidate `sourcesKeys.all` |

- **Correctness comes from invalidate-on-reconnect, not from replay.** The server persists recent events and honors `Last-Event-ID`, but treat that as a latency optimization. On every `open` after a disconnect, invalidate the queries that live updates maintain. A replayed event that also arrives through invalidation is harmless; a dropped event that invalidation would have covered is a view that silently stops updating.

  > [!NOTE]
  > This mirrors the reasoning in ADR-0003 for preferring polling over `LISTEN`/`NOTIFY`: a delivery mechanism that can lose messages may accelerate a correct design, but it must not be the thing the design depends on.

- **SSE is never the only path to a state change.** Every mutation's HTTP response is authoritative, and every view is reachable by a plain fetch. The stream makes the UI live; it does not make it correct.
- **Server side.** `axum::response::sse::Sse` with `KeepAlive` emitting a comment roughly every 15 seconds, a `retry:` field so the client's backoff is server-controlled, and one `sse.connection` span per connection as the observability design specifies.
- **Exclude `text/event-stream` from response compression**, and set `X-Accel-Buffering: no` on the stream response. See [Two ways this fails quietly](#two-ways-this-fails-quietly).
- **Accept one connection per browser tab in v1.** Do not build connection sharing yet; see the same section for why this is a bounded risk and what the escape hatch is.

### Two ways this fails quietly

Both of these produce a feature that works perfectly in development and is broken or destructive in production.

#### 1. The HTTP/1.1 six-connection limit

Browsers cap concurrent HTTP/1.1 connections per origin at six, and an SSE stream holds one open indefinitely. The failure mode is not a stream error — it is that the **seventh request of any kind to that origin queues with no timeout and no error** until a slot frees. A user with several tabs open sees the whole application hang, and nothing in the logs says why. Chrome and Firefox have both marked this "won't fix".

Over HTTP/2 the limit effectively disappears: the browser and server negotiate concurrent streams, defaulting to around 100.

> [!IMPORTANT]
> Run production behind the `caddy` service with TLS so the browser negotiates HTTP/2. The architecture lists `caddy` as optional; for this decision it is optional only for single-tab use. Without it, the browser speaks HTTP/1.1 — browsers do not use h2c — and the application will appear to freeze once a user opens roughly six tabs. Document this in the runbook and in the deployment README, not only here.

#### 2. Buffering and compression

An SSE response that is buffered or compressed is a stream that arrives in one batch, or not at all.

- **Compression is the subtler of the two.** A compressor accumulates input before it emits output, so a `CompressionLayer` applied to the stream holds events even when every proxy in front is configured correctly. Exclude `text/event-stream` explicitly.
- **Caddy** recognizes `text/event-stream` and flushes immediately. Set `flush_interval -1` on the `reverse_proxy` anyway.
- **nginx**, if an operator substitutes it, buffers proxied responses by default. It needs `proxy_buffering off`, `proxy_cache off`, `proxy_http_version 1.1`, `gzip off`, and read and send timeouts well above the heartbeat interval. nginx honors the `X-Accel-Buffering: no` response header, which is why the application sets it: it lets the app fix its own streaming without the operator editing proxy config.

## Consequences

### What you gain

- **One channel, one cache, one source of truth.** Pushed data and fetched data land in the same place, so a component cannot read a stale copy of something the stream just updated.
- **Authentication is free.** The session cookie is sent automatically. No token in a query string, no header workaround, no separate credential lifecycle for the stream.
- **Reconnection is native.** `EventSource` reconnects on its own and sends `Last-Event-ID` without application code. The server controls the backoff through `retry:`.
- **The server side is trivial.** A `broadcast::Receiver` mapped to an event stream. No frame protocol, no ping/pong, no subprotocol negotiation, no per-connection write state beyond the subscription.
- **Testing is straightforward.** Injecting a fake `EventSource` exercises every handler, and because handlers only call `setQueryData` and `invalidateQueries`, their effects are assertable without rendering anything.
- **No polling anywhere.** Jobs can use `staleTime: 0` with no `refetchInterval`, so an idle application generates no traffic.

### What it costs you

- **HTTP/2 becomes a production requirement**, which promotes Caddy from optional convenience to recommended component. The alternative is a documented multi-tab limit.
- **One connection per tab.** Six tabs is the practical ceiling on HTTP/1.1 and a non-issue on HTTP/2, but it is still one server-side subscription and one `sse.connection` span per tab.
- **Unidirectional.** Anything the client needs to send is a REST call. That fits this application exactly, and it would not fit one with client-to-server streaming.
- **Invalidate-on-reconnect costs a burst of requests.** After a network blip, several queries refetch at once. This is the price of not trusting replay, and it is the right price.
- **No delivery guarantee.** Events can be lost between the broadcast channel and the browser, which is why the correctness rule above exists. A future contributor who makes a state transition observable *only* through SSE reintroduces the bug the rule prevents.
- **Coupled to a single API instance.** The `broadcast` channel is in-process, as ADR-0003 already states. A second instance would deliver events only to clients connected to the instance that ran the job.
- **Two silent-failure modes to keep configured.** Compression exclusion and proxy flushing are the kind of settings that get lost in a config refactor and produce a "real-time feature stopped working" bug with no error anywhere.

### Follow-up work this decision creates

1. **Add a test asserting `text/event-stream` responses are not compressed.** This is the failure most likely to be reintroduced by a later middleware change.
2. **Implement invalidate-on-reconnect** in the SSE provider, with the invalidation set defined next to the handler table so the two stay together.
3. **Add an integration test for a dropped connection**: disconnect mid-job, reconnect, and assert the UI converges to the correct state even when replay returns nothing.
4. **Document the HTTP/2 requirement** in the deployment README and the runbook, with the multi-tab symptom described so an operator can recognize it.
5. **Set `flush_interval -1`** in the shipped Caddy configuration and keep the SSE-specific settings commented as such, so they survive edits.
6. **Emit `retry:` from the server** with a configurable value rather than relying on the browser default.
7. **Record `sse.events_sent` on disconnect**, as the observability design specifies, to make a silently stalled stream visible in logs.

## Alternatives considered

| Alternative | Direction | Native reconnect and replay | Counts against HTTP/1.1 pool | Outcome |
|---|---|---|---|---|
| One SSE stream into the Query cache | Server to client | Yes | Yes | **Chosen** |
| WebSocket | Bidirectional | No, build it | No, separate pool | Rejected. Buys bidirectionality this app does not use. |
| Polling with `refetchInterval` | Client pull | N/A | No long-lived connection | Rejected as primary; retained as the fallback. |
| Long polling | Client pull | No | Yes | Rejected. SSE's problems with none of its benefits. |
| `fetch`-based SSE client | Server to client | No, build it | Yes | Rejected. Solves a problem this design does not have. |

### WebSocket

Axum supports WebSockets in core, the handshake is an HTTP request so the session cookie authenticates it, and a WebSocket has one genuine advantage here: browsers track WebSocket connections in a **separate pool from HTTP/1.1 requests**, so a long-lived WebSocket does not starve ordinary API calls the way an SSE stream can. Sources disagree on the exact per-origin WebSocket cap — figures between 6 and 30 per origin appear, with a global limit around 255 — so it is not unlimited, but it does not consume the six slots that `fetch` needs.

It was rejected because that advantage is the same thing HTTP/2 provides, and everything else is worse for this application:

1. **Bidirectionality is unused.** Every client-to-server action is already a REST endpoint with problem+json errors, idempotency keys, and OpenAPI-typed request bodies. Sending mutations over a socket would mean inventing a message protocol, its error semantics, and its typing, in parallel with the API that already exists.
2. **Reconnection and replay become application code.** There is no `Last-Event-ID` equivalent and no automatic reconnect. The client would own backoff, resume cursors, and connection-state machinery that `EventSource` provides for free.
3. **The server gets heavier.** Ping/pong liveness, close-frame handling, and per-connection write buffers replace a `broadcast::Receiver` mapped to a stream.

If client-to-server streaming ever becomes a requirement, this is the alternative, and moving to it is a contained change because no component touches the transport today.

### Polling with `refetchInterval`

This is the honest baseline and deserves credit: a two-second `refetchInterval` on the jobs list would work, needs no server support, has no connection-limit problem, and no buffering footguns. For the jobs list alone it would be nearly indistinguishable.

It was rejected as the primary mechanism because progress during a download needs finer granularity than polling provides without becoming wasteful, and because `chapter.new` and `source.updated` would require polling views that are otherwise static — turning an idle application into a constant stream of requests against a single-VM deployment.

It remains the fallback, and that matters: because every view is reachable by a plain fetch and every mutation response is authoritative, adding `refetchInterval` to specific queries is a small, local change if SSE proves unreliable in some deployment.

### Long polling

Rejected. It occupies a connection from the same HTTP/1.1 pool, needs the same proxy configuration, and requires application-level resume logic — SSE's operational constraints without its native reconnect or replay.

### A `fetch`-based SSE client

Libraries that implement SSE over `fetch` allow custom headers, `POST` bodies, and richer error handling. Rejected because the only motivation would be sending an `Authorization` header, and ADR-0005 uses cookies precisely so that is unnecessary. Adopting one would trade away native reconnection for a capability this design does not need.

## Revisit this decision when

- **Client-to-server streaming becomes a requirement.** Then WebSocket, and the transport swap is contained because handlers are the only thing that touch the cache.
- **A second API instance is needed.** The in-process `broadcast` channel is the blocker, and this must be revisited together with ADR-0003 rather than separately.
- **Operators report the application hanging with several tabs open.** That is the six-connection limit, and the answer is HTTP/2 first, connection sharing via `SharedWorker` or `BroadcastChannel` leader election second.
- **Invalidate-on-reconnect causes a noticeable request burst** on flaky networks. Then make replay authoritative for a bounded window and narrow the invalidation set — a change that needs server-side event retention guarantees first.

## References

- [MDN: Using server-sent events](https://developer.mozilla.org/docs/Web/API/Server-sent_events/Using_server-sent_events)
- [MDN: `EventSource`](https://developer.mozilla.org/docs/Web/API/EventSource)
- [The pitfalls of EventSource over HTTP/1.1](https://textslashplain.com/2019/12/04/the-pitfalls-of-eventsource-over-http-1-1/) — the six-connection limit
- [Diagnosing the six-connection limit per origin](https://www.server-sent-events.com/sse-protocol-fundamentals-architecture/http2-and-http3-for-event-streams/diagnosing-the-six-connection-limit-per-origin/) — directional
- [Sharing one SSE connection across tabs](https://www.server-sent-events.com/frontend-consumption-client-patterns/sharing-one-sse-connection-across-tabs/) — the deferred escape hatch
- [Proxy and CDN configuration for SSE](https://www.server-sent-events.com/sse-protocol-fundamentals-architecture/proxy-and-cdn-configuration-for-sse/) — directional
- [Caddy issue 4247: response buffer flushing and SSE](https://github.com/leafac/caddy-express-sse)
- [WebSocket connection limits](https://websocket.org/guides/connection-limits/) — directional; figures vary by source
- [`axum::response::sse`](https://docs.rs/axum/latest/axum/response/sse/index.html)
