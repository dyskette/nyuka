/**
 * The single EventSource (ADR-0010).
 *
 * Opened in a provider mounted after authentication. No component constructs
 * an EventSource, and no component subscribes to this one: handlers write only
 * into the Query cache, addressed through feature key factories (ADR-0009).
 *
 * Correctness comes from invalidate-on-reconnect, NOT from replay. The server
 * honours Last-Event-ID, but treat that as a latency optimisation: on every
 * `open` after a disconnect, invalidate the queries that live updates
 * maintain. A replayed event that also arrives via invalidation is harmless; a
 * dropped event that invalidation would have covered is a view that silently
 * stops updating.
 *
 * TODO(scaffold): handler table, reconnect invalidation set, fake EventSource
 * for tests.
 */
export {}
