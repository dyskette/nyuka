/**
 * An RFC 9457 problem document, as the server sends it.
 *
 * Every error from `/api/v1` has this shape; nothing else does. Modelling it
 * means a component can branch on *which* failure occurred rather than on a
 * status code alone — the difference between "sign in" and "this URL does not
 * exist" is a `type`, not a number, because both are 404-adjacent from the
 * client's side.
 */
export interface Problem {
  type: string
  title: string
  status: number
  detail?: string
  instance?: string
  errors?: FieldError[]
  /** Present on server faults, where `detail` deliberately is not. */
  trace_id?: string
}

export interface FieldError {
  /** A JSON Pointer into the request body, such as `/check_interval_secs`. */
  field: string
  message: string
}

/** Problem types the UI branches on. Others are handled generically. */
export const ProblemType = {
  Unauthenticated: '/problems/unauthenticated',
  Forbidden: '/problems/forbidden',
  NoSuchEndpoint: '/problems/no-such-endpoint',
  NotFound: '/problems/not-found',
  AuthDisabled: '/problems/auth-disabled',
  UnsupportedCapability: '/problems/unsupported-capability',
  InsufficientStorage: '/problems/insufficient-storage',
  RateLimited: '/problems/rate-limited',
} as const

export function isProblem(value: unknown): value is Problem {
  if (typeof value !== 'object' || value === null) return false
  const candidate = value as Record<string, unknown>
  return typeof candidate.type === 'string' && typeof candidate.status === 'number'
}

/**
 * Whether a failure means the session is gone.
 *
 * Checked by type rather than by status: a 401 is unambiguous today, but the
 * type is what the server actually promises, and branching on it survives a
 * status changing for an unrelated reason.
 */
export function isUnauthenticated(value: unknown): boolean {
  return isProblem(value) && value.type === ProblemType.Unauthenticated
}

/**
 * The message to show for a failure.
 *
 * Prefers `detail`, which the server writes for the caller on a 4xx and
 * deliberately withholds on a 5xx — there it is a fixed string and the
 * `trace_id` is the useful part. Returns `null` when there is nothing
 * specific to say, so a caller renders its own translated fallback rather
 * than this module inventing English.
 */
export function problemMessage(value: unknown): string | null {
  if (!isProblem(value)) return null
  return value.detail ?? value.title ?? null
}

/**
 * Field errors keyed by the form field they belong to.
 *
 * The server sends JSON Pointers (`/check_interval_secs`); a form knows its
 * fields by name. Stripping the leading slash is that translation, done once
 * here rather than at each form.
 */
export function fieldErrors(value: unknown): Record<string, string> {
  if (!isProblem(value) || !value.errors) return {}
  return Object.fromEntries(
    value.errors.map((error) => [error.field.replace(/^\//, ''), error.message]),
  )
}
