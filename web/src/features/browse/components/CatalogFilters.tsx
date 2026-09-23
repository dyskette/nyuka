import { Trans, useLingui } from '@lingui/react/macro'
import { useState } from 'react'
import type { Filter, FilterState, FilterValue } from '../lib/filters'

export interface CatalogFiltersProps {
  filters: Filter[]
  state: FilterState
  /** Called with the whole next state, so the caller writes one URL. */
  onChange: (next: FilterState) => void
}

/**
 * A source's own search filters, rendered from its declaration.
 *
 * Collapsed behind a disclosure, because a source can declare a dozen and the
 * common case is searching by name. Open when anything is set, so a filtered
 * view never looks unfiltered.
 */
export function CatalogFilters({ filters, state, onChange }: CatalogFiltersProps) {
  const active = Object.keys(state).length
  const [open, setOpen] = useState(active > 0)

  if (filters.length === 0) return null

  function set(id: string, value: FilterValue | undefined) {
    const next = { ...state }
    if (value === undefined) delete next[id]
    else next[id] = value
    onChange(next)
  }

  return (
    <section className="border-border border-b">
      <div className="px-cell flex h-row items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen((previous) => !previous)}
          aria-expanded={open}
          className="text-muted-foreground hover:text-foreground flex items-center gap-1 text-xs"
        >
          <span aria-hidden="true">{open ? '▾' : '▸'}</span>
          <Trans>Filters</Trans>
        </button>

        {active > 0 && (
          <>
            <span className="bg-accent-soft tabular rounded-sm px-1.5 py-0.5 text-xs">
              {active}
            </span>
            <button
              type="button"
              onClick={() => onChange({})}
              className="text-muted-foreground hover:text-foreground text-xs"
            >
              <Trans>Clear</Trans>
            </button>
          </>
        )}
      </div>

      {open && (
        <div className="p-panel grid gap-3 sm:grid-cols-2">
          {filters.map((filter) => (
            <Control key={filter.id} filter={filter} value={state[filter.id]} onSet={set} />
          ))}
        </div>
      )}
    </section>
  )
}

function Control({
  filter,
  value,
  onSet,
}: {
  filter: Filter
  value: FilterValue | undefined
  onSet: (id: string, value: FilterValue | undefined) => void
}) {
  const { t } = useLingui()

  switch (filter.kind) {
    case 'unsupported':
      return (
        <p className="text-muted-foreground text-xs">
          {/* Named, because a reader who knows a source filters by year should
              be told it is not available rather than left looking. */}
          <Trans>
            {filter.title} is not available here ({filter.declaredType})
          </Trans>
        </p>
      )

    case 'text': {
      const current = value && 'Text' in value ? value.Text.value : ''
      return (
        <label className="flex flex-col gap-1 text-xs">
          {filter.title}
          <input
            type="text"
            defaultValue={current}
            aria-label={filter.title}
            // On blur, not per keystroke: each change is a request to a
            // third-party site.
            onBlur={(event) => {
              const next = event.target.value.trim()
              onSet(filter.id, next === '' ? undefined : { Text: { id: filter.id, value: next } })
            }}
            className="border-border bg-surface h-7 rounded-sm border px-2 text-sm"
          />
        </label>
      )
    }

    case 'check': {
      const checked = value !== undefined && 'Check' in value && value.Check.value === 1
      return (
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            className="accent-accent size-4"
            checked={checked}
            onChange={(event) =>
              onSet(
                filter.id,
                event.target.checked ? { Check: { id: filter.id, value: 1 } } : undefined,
              )
            }
          />
          {filter.title}
        </label>
      )
    }

    case 'select': {
      const current = value && 'Select' in value ? value.Select.value : ''
      return (
        <label className="flex flex-col gap-1 text-xs">
          {filter.title}
          <select
            value={current}
            aria-label={filter.title}
            onChange={(event) =>
              onSet(
                filter.id,
                event.target.value === ''
                  ? undefined
                  : { Select: { id: filter.id, value: event.target.value } },
              )
            }
            className="text-foreground bg-surface border-border h-7 rounded-sm border px-1 text-sm"
          >
            {/* An unset filter shows blank rather than the first option:
                preselecting one would narrow a listing nobody asked to narrow. */}
            <option value="">—</option>
            {filter.options.map((option, index) => (
              <option key={filter.values[index] ?? option} value={filter.values[index] ?? option}>
                {option}
              </option>
            ))}
          </select>
        </label>
      )
    }

    case 'sort': {
      const current = value && 'Sort' in value ? value.Sort : undefined
      return (
        <div className="flex flex-col gap-1 text-xs">
          <span>{filter.title}</span>
          <div className="flex items-center gap-2">
            <select
              value={current ? String(current.index) : ''}
              aria-label={filter.title}
              onChange={(event) =>
                onSet(
                  filter.id,
                  event.target.value === ''
                    ? undefined
                    : {
                        Sort: {
                          id: filter.id,
                          index: Number(event.target.value),
                          ascending: current?.ascending ?? false,
                        },
                      },
                )
              }
              className="text-foreground bg-surface border-border h-7 flex-1 rounded-sm border px-1 text-sm"
            >
              <option value="">—</option>
              {filter.options.map((option, index) => (
                <option key={option} value={index}>
                  {option}
                </option>
              ))}
            </select>

            {/* Only when the source says the order can be reversed. Offering
                it otherwise sends a flag the source ignores, and the listing
                does not change — which reads as a broken control. */}
            {filter.canAscend && current !== undefined && (
              <button
                type="button"
                onClick={() =>
                  onSet(filter.id, {
                    Sort: { id: filter.id, index: current.index, ascending: !current.ascending },
                  })
                }
                aria-label={current.ascending ? t`Sort descending` : t`Sort ascending`}
                className="border-border hover:bg-surface-raised h-7 rounded-sm border px-2"
              >
                <span aria-hidden="true">{current.ascending ? '▲' : '▼'}</span>
              </button>
            )}
          </div>
        </div>
      )
    }

    case 'multi-select': {
      const current =
        value && 'MultiSelect' in value ? value.MultiSelect : { included: [], excluded: [] }
      const included = new Set(current.included)
      const excluded = new Set(current.excluded)

      return (
        <fieldset className="flex flex-col gap-1 text-xs">
          <legend>{filter.title}</legend>
          <div className="flex flex-wrap gap-2 pt-1">
            {filter.options.map((option, index) => {
              const sent = filter.values[index] ?? option
              const state = included.has(sent) ? 'in' : excluded.has(sent) ? 'out' : 'off'

              return (
                <button
                  key={sent}
                  type="button"
                  // Three states on one control when the source allows
                  // exclusion, because a checkbox has two and the third would
                  // need a second control per option.
                  aria-pressed={state !== 'off'}
                  aria-label={
                    state === 'in'
                      ? t`${option}: included`
                      : state === 'out'
                        ? t`${option}: excluded`
                        : t`${option}: not filtered`
                  }
                  onClick={() => {
                    const nextIncluded = new Set(included)
                    const nextExcluded = new Set(excluded)
                    if (state === 'off') {
                      nextIncluded.add(sent)
                    } else if (state === 'in') {
                      nextIncluded.delete(sent)
                      if (filter.canExclude) nextExcluded.add(sent)
                    } else {
                      nextExcluded.delete(sent)
                    }
                    onSet(filter.id, {
                      MultiSelect: {
                        id: filter.id,
                        // Ordered by the declaration, so the same selection
                        // always produces the same URL.
                        included: filter.values.filter((v) => nextIncluded.has(v)),
                        excluded: filter.values.filter((v) => nextExcluded.has(v)),
                      },
                    })
                  }}
                  className={`rounded-sm border px-1.5 py-0.5 ${
                    state === 'in'
                      ? 'border-accent bg-accent-soft'
                      : state === 'out'
                        ? 'border-danger text-danger line-through'
                        : 'border-border'
                  }`}
                >
                  {option}
                </button>
              )
            })}
          </div>
        </fieldset>
      )
    }
  }
}
