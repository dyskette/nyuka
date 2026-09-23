import { i18n } from '@lingui/core'
import { Trans, useLingui } from '@lingui/react/macro'
import { Link } from '@tanstack/react-router'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'

type FollowSummary = components['schemas']['FollowSummaryDto']

export interface FollowTableProps {
  follows: FollowSummary[]
  /** Follow ids with a check or edit in flight. */
  busy?: ReadonlySet<string>
  onCheckNow: (followId: string) => void
  onUnfollow: (followId: string) => void
  onIntervalChange: (mangaId: string, seconds: number) => void
  onAutoDownloadChange: (mangaId: string, enabled: boolean) => void
}

/**
 * The intervals offered, in seconds.
 *
 * A fixed set rather than a free number field: every value here is a promise
 * to hit someone else's site on a schedule, and a text box invites "60".
 */
const INTERVALS = [3600, 6 * 3600, 12 * 3600, 24 * 3600, 7 * 24 * 3600] as const

/**
 * Followed series and what each is waiting on.
 *
 * The one column a reader acts on is `missing_count` — chapters with no file,
 * which is what a follow exists to produce. Everything else is the schedule
 * that produces it.
 */
export function FollowTable({
  follows,
  busy,
  onCheckNow,
  onUnfollow,
  onIntervalChange,
  onAutoDownloadChange,
}: FollowTableProps) {
  if (follows.length === 0) return <NoFollows />

  return (
    <table className="w-full border-collapse text-sm">
      <caption className="sr-only">
        <Trans>Series you follow</Trans>
      </caption>
      <thead>
        <tr className="border-border text-muted-foreground border-b text-left text-xs">
          <Th>
            <Trans>Series</Trans>
          </Th>
          <Th>
            <Trans>Source</Trans>
          </Th>
          <Th align="right">
            <Trans>Missing</Trans>
          </Th>
          <Th>
            <Trans>Every</Trans>
          </Th>
          <Th>
            <Trans>Auto-download</Trans>
          </Th>
          <Th>
            <Trans>Last checked</Trans>
          </Th>
          <th scope="col" className="px-cell h-row w-0">
            <span className="sr-only">
              <Trans>Actions</Trans>
            </span>
          </th>
        </tr>
      </thead>
      <tbody>
        {follows.map((follow) => (
          <Row
            key={follow.id}
            follow={follow}
            busy={busy?.has(follow.id) ?? false}
            onCheckNow={onCheckNow}
            onUnfollow={onUnfollow}
            onIntervalChange={onIntervalChange}
            onAutoDownloadChange={onAutoDownloadChange}
          />
        ))}
      </tbody>
    </table>
  )
}

function Th({ children, align = 'left' }: { children: React.ReactNode; align?: 'left' | 'right' }) {
  return (
    <th
      scope="col"
      className={`px-cell h-row font-medium ${align === 'right' ? 'text-right' : ''}`}
    >
      {children}
    </th>
  )
}

function Row({
  follow,
  busy,
  onCheckNow,
  onUnfollow,
  onIntervalChange,
  onAutoDownloadChange,
}: {
  follow: FollowSummary
  busy: boolean
  onCheckNow: (followId: string) => void
  onUnfollow: (followId: string) => void
  onIntervalChange: (mangaId: string, seconds: number) => void
  onAutoDownloadChange: (mangaId: string, enabled: boolean) => void
}) {
  const { t } = useLingui()

  return (
    <tr className="border-border hover:bg-surface-raised border-b">
      <td className="px-cell h-row max-w-0">
        <Link
          to="/library/$mangaId"
          params={{ mangaId: follow.manga_id }}
          search={{ tab: 'chapters' }}
          className="block truncate font-medium"
        >
          {follow.manga_title}
        </Link>
      </td>
      <td className="px-cell text-muted-foreground h-row truncate text-xs">{follow.source_name}</td>
      <td className="px-cell tabular h-row text-right">
        {/* Zero is not emphasised: a caught-up follow is the normal state, and
            a bold 0 in every row would make the column unreadable. */}
        {follow.missing_count > 0 ? (
          <span className="text-accent font-medium">{follow.missing_count}</span>
        ) : (
          <span className="text-muted-foreground">0</span>
        )}
      </td>
      <td className="px-cell h-row">
        <select
          value={follow.check_interval_secs}
          disabled={busy}
          onChange={(event) => onIntervalChange(follow.manga_id, Number(event.target.value))}
          aria-label={t`Check ${follow.manga_title} every`}
          className="text-foreground bg-surface border-border h-6 rounded-sm border px-1 text-xs"
        >
          {/*
            A server-set interval outside the offered set still has to render,
            or the select would silently show the first option and an edit to
            any other field would overwrite it.
          */}
          {!INTERVALS.includes(follow.check_interval_secs as (typeof INTERVALS)[number]) && (
            <option value={follow.check_interval_secs}>
              {formatInterval(follow.check_interval_secs)}
            </option>
          )}
          {INTERVALS.map((seconds) => (
            <option key={seconds} value={seconds}>
              {formatInterval(seconds)}
            </option>
          ))}
        </select>
      </td>
      <td className="px-cell h-row">
        <input
          type="checkbox"
          className="accent-accent size-4 align-middle"
          checked={follow.auto_download}
          disabled={busy}
          onChange={(event) => onAutoDownloadChange(follow.manga_id, event.target.checked)}
          aria-label={t`Download new chapters of ${follow.manga_title} automatically`}
        />
      </td>
      <td className="px-cell text-muted-foreground h-row text-xs">
        {follow.last_checked_at !== null && follow.last_checked_at !== undefined ? (
          <time dateTime={follow.last_checked_at}>{relativeTime(follow.last_checked_at)}</time>
        ) : (
          <Trans>Never</Trans>
        )}
      </td>
      <td className="px-cell h-row">
        <span className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => onCheckNow(follow.id)}
            disabled={busy}
            aria-label={t`Check ${follow.manga_title} now`}
            className="text-accent hover:bg-accent-soft rounded-sm px-2 py-0.5 text-xs disabled:opacity-50"
          >
            <Trans>Check now</Trans>
          </button>
          <button
            type="button"
            onClick={() => onUnfollow(follow.id)}
            disabled={busy}
            aria-label={t`Stop following ${follow.manga_title}`}
            className="text-muted-foreground hover:text-danger rounded-sm px-2 py-0.5 text-xs disabled:opacity-50"
          >
            <Trans>Unfollow</Trans>
          </button>
        </span>
      </td>
    </tr>
  )
}

/**
 * An interval, in whole hours or days.
 *
 * `Intl.NumberFormat` with a unit, so a locale that does not say "6 hours"
 * gets its own form and its own plural rules.
 *
 * The locale comes from the active catalog, never from `undefined` — that
 * resolves to the *platform's* locale, which is not necessarily the one the
 * application is running in. `relativeTime` had exactly this bug.
 */
function formatInterval(seconds: number, locale: string = i18n.locale): string {
  const hours = Math.round(seconds / 3600)
  const [value, unit]: [number, 'day' | 'hour'] =
    hours >= 24 && hours % 24 === 0 ? [hours / 24, 'day'] : [hours, 'hour']

  return new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: 'long' }).format(value)
}

function NoFollows() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>You follow nothing yet</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>
          Following a series checks it on a schedule and can download new chapters as they appear.
          Open a series in your library to follow it.
        </Trans>
      </p>
    </div>
  )
}
