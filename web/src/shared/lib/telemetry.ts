/**
 * Phase 1 telemetry (ADR-0013): W3C `traceparent` propagation and error
 * reporting, with no OpenTelemetry web SDK.
 *
 * The SDK is deferred to Phase 2 because assembled carelessly it is 200 KB+
 * before first interaction, and the two things that actually matter —
 * correlation and crash visibility — cost a few hundred bytes without it.
 *
 * IMPORTANT (ADR-0015): the trace ID must satisfy W3C Trace Context Level 2,
 * which OpenTelemetry is adopting as the basis for consistent sampling. That
 * means the random flag set and the low 56 bits genuinely random, from
 * `crypto.getRandomValues` and never `Math.random`. Getting this wrong
 * produces IDs that correlate fine today and silently break consistent
 * sampling the moment an exporter is enabled.
 *
 * TODO(scaffold): implement, and cover with a test asserting format,
 * the random flag, and entropy source.
 */
export const TRACE_FLAG_SAMPLED = 0x01
/** Trace Context Level 2: the trace-id was generated with sufficient randomness. */
export const TRACE_FLAG_RANDOM = 0x02
