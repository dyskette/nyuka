import { Trans, useLingui } from '@lingui/react/macro'
import { useEffect, useState } from 'react'

export interface LibraryFilters {
  q?: string | undefined
  status: string
  source: string
}

export interface LibraryToolbarProps {
  filters: LibraryFilters
  /** Every distinct value present on the loaded page, for the two selects. */
  sources: string[]
  statuses: string[]
  /** Number of rows after filtering, for the count readout. */
  shown: number
  total: number
  onChange: (next: Partial<LibraryFilters>) => void
}

/**
 * The library's filter bar.
 *
 * The controls write to the URL rather than to local state, so a filtered
 * view is shareable and the back button undoes a filter (ADR-0008). The
 * search box is the exception in one respect only — see [`SearchBox`].
 *
 * # The mockup's "Unread" pill is not here
 *
 * It filters on how many chapters the reader has not read, and nothing
 * records that: the API tracks what is *downloaded*, not what is read. A
 * control that silently filtered on the wrong number would be worse than its
 * absence, so it arrives with read progress. See TODO.md.
 */
export function LibraryToolbar({
  filters,
  sources,
  statuses,
  shown,
  total,
  onChange,
}: LibraryToolbarProps) {
  const { t } = useLingui()

  return (
    <div className="border-border px-cell flex h-row shrink-0 items-center gap-2 border-b">
      <SearchBox
        value={filters.q ?? ''}
        placeholder={t`Search ${total} titles`}
        onChange={(q) => onChange({ q })}
      />

      <Select
        label={t`Status`}
        value={filters.status}
        options={statuses}
        anyLabel={t`Any`}
        onChange={(status) => onChange({ status })}
      />
      <Select
        label={t`Source`}
        value={filters.source}
        options={sources}
        anyLabel={t`All`}
        onChange={(source) => onChange({ source })}
      />

      {/* `aria-live` so a screen reader hears the result of a filter it just
          applied. `polite`, because it must not interrupt typing. */}
      <span className="text-muted-foreground tabular ml-auto text-xs" aria-live="polite">
        {shown === total ? (
          <Trans>{total} titles</Trans>
        ) : (
          <Trans>
            {shown} of {total}
          </Trans>
        )}
      </span>
    </div>
  )
}

/**
 * The search box.
 *
 * Held in local state and pushed to the URL after a pause. Writing on every
 * keystroke would put one history entry per character — the back button would
 * then delete the query one letter at a time, which is not what anyone means
 * by "go back".
 *
 * The URL still wins: the effect below resets the local value whenever the
 * prop changes, so the back button and a pasted link both take effect.
 */
function SearchBox({
  value,
  placeholder,
  onChange,
}: {
  value: string
  placeholder: string
  onChange: (value: string) => void
}) {
  const [draft, setDraft] = useState(value)

  useEffect(() => setDraft(value), [value])

  useEffect(() => {
    if (draft === value) return
    const timer = setTimeout(() => onChange(draft), 200)
    return () => clearTimeout(timer)
  }, [draft, value, onChange])

  return (
    <input
      type="search"
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      placeholder={placeholder}
      // A placeholder is not a label: it disappears on focus, and a screen
      // reader in forms mode may never announce it at all.
      aria-label={placeholder}
      className="border-border bg-surface focus-visible:border-border-strong h-7 w-64 rounded-sm border px-2 text-sm"
    />
  )
}

/**
 * One filter dropdown.
 *
 * A native `<select>`: it is keyboard-operable, announced correctly, and
 * renders as the platform's own picker on a phone. A custom listbox would
 * have to reimplement all of that to be no better.
 */
function Select({
  label,
  value,
  options,
  anyLabel,
  onChange,
}: {
  label: string
  value: string
  options: string[]
  anyLabel: string
  onChange: (value: string) => void
}) {
  return (
    <label className="text-muted-foreground flex items-center gap-1 text-xs">
      {label}
      <select
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className="text-foreground bg-surface border-border h-7 rounded-sm border px-1 text-xs"
      >
        {/* The empty string is "no filter". A sentinel like "all" would be
            indistinguishable from a source actually called that. */}
        <option value="">{anyLabel}</option>
        {options.map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
      </select>
    </label>
  )
}
