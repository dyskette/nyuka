import { Trans, useLingui } from '@lingui/react/macro'
import { useState } from 'react'
import type { Value } from '../lib/postcard'
import type { EditableSetting, SettingGroup } from '../lib/settings'

export interface SourceSettingsProps {
  groups: SettingGroup[]
  /** Current values by key. A key absent means the source's own default.  */
  values: ReadonlyMap<string, Value>
  /** Keys with a save in flight. */
  saving?: ReadonlySet<string>
  onChange: (setting: EditableSetting, value: Value) => void
}

/**
 * A source's own settings, rendered from its declaration.
 *
 * The server passes the declaration through without interpreting it, so
 * everything rendered here comes from a third-party package. Controls this
 * build cannot offer are named rather than hidden: a reader who cannot find a
 * setting they know a source has should be told it exists and is not
 * available, not left looking for it.
 */
export function SourceSettings({ groups, values, saving, onChange }: SourceSettingsProps) {
  if (groups.length === 0) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>This source has no settings.</Trans>
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      {groups.map((group, index) => (
        <section key={group.title ?? `group-${index}`}>
          {group.title !== null && (
            <h4 className="text-muted-foreground px-cell pb-1 text-xs font-medium">
              {group.title}
            </h4>
          )}
          <ul className="border-border flex flex-col rounded-md border">
            {group.settings.map((setting) => (
              <li
                key={setting.kind === 'unsupported' ? setting.title : setting.key}
                className="border-border px-cell flex min-h-row items-center gap-3 border-b py-1.5 last:border-b-0"
              >
                {setting.kind === 'unsupported' ? (
                  <Unsupported declaredType={setting.declaredType} title={setting.title} />
                ) : (
                  <Control
                    setting={setting}
                    value={values.get(setting.key)}
                    saving={saving?.has(setting.key) ?? false}
                    onChange={onChange}
                  />
                )}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  )
}

function Unsupported({ declaredType, title }: { declaredType: string; title: string }) {
  return (
    <>
      <span className="text-muted-foreground min-w-0 flex-1 truncate text-sm">{title}</span>
      <span className="text-muted-foreground shrink-0 text-xs">
        {/* The kind is named: "not supported" without saying what leaves a
            reader unable to tell a gap from a bug. */}
        <Trans>Not supported yet ({declaredType})</Trans>
      </span>
    </>
  )
}

function Control({
  setting,
  value,
  saving,
  onChange,
}: {
  setting: EditableSetting
  value: Value | undefined
  saving: boolean
  onChange: (setting: EditableSetting, value: Value) => void
}) {
  const { t } = useLingui()
  const label = t`${setting.title}`

  switch (setting.kind) {
    case 'switch':
      return (
        <>
          <span className="min-w-0 flex-1 truncate text-sm">{setting.title}</span>
          <input
            type="checkbox"
            className="accent-accent size-4 shrink-0"
            checked={value === true}
            disabled={saving}
            onChange={(event) => onChange(setting, event.target.checked)}
            aria-label={label}
          />
        </>
      )

    case 'text':
      return (
        <>
          <span className="min-w-0 flex-1 truncate text-sm">{setting.title}</span>
          <TextControl
            label={label}
            value={typeof value === 'string' ? value : ''}
            saving={saving}
            onCommit={(next) => onChange(setting, next)}
          />
        </>
      )

    case 'select':
    case 'segment':
      return (
        <>
          <span className="min-w-0 flex-1 truncate text-sm">{setting.title}</span>
          <select
            className="text-foreground bg-surface border-border h-7 shrink-0 rounded-sm border px-1 text-xs"
            value={typeof value === 'string' ? value : ''}
            disabled={saving}
            onChange={(event) => onChange(setting, event.target.value)}
            aria-label={label}
          >
            {/*
              An unset setting shows a blank option rather than the first one.
              Preselecting an option would claim a value the source has not
              stored, and the reader would have no way to get back to "unset".
            */}
            {!setting.values.includes(typeof value === 'string' ? value : '') && (
              <option value="">—</option>
            )}
            {setting.options.map((option, index) => (
              <option key={setting.values[index] ?? option} value={setting.values[index] ?? option}>
                {option}
              </option>
            ))}
          </select>
        </>
      )

    case 'multi-select': {
      const selected = new Set(Array.isArray(value) ? value : [])
      return (
        <fieldset className="min-w-0 flex-1">
          <legend className="text-sm">{setting.title}</legend>
          <div className="flex flex-wrap gap-2 pt-1">
            {setting.options.map((option, index) => {
              const stored = setting.values[index] ?? option
              return (
                <label key={stored} className="flex items-center gap-1 text-xs">
                  <input
                    type="checkbox"
                    className="accent-accent size-3.5"
                    checked={selected.has(stored)}
                    disabled={saving}
                    onChange={(event) => {
                      const next = new Set(selected)
                      if (event.target.checked) next.add(stored)
                      else next.delete(stored)
                      // Ordered by the declaration rather than by when each
                      // was ticked, so saving twice with the same boxes
                      // checked produces the same bytes.
                      onChange(
                        setting,
                        setting.values.filter((candidate) => next.has(candidate)),
                      )
                    }}
                  />
                  {option}
                </label>
              )
            })}
          </div>
        </fieldset>
      )
    }
  }
}

/**
 * A text setting.
 *
 * Committed on blur or Enter rather than per keystroke: each save is a request
 * and a write into the source's own store, and typing a token should not be
 * thirty of them.
 */
function TextControl({
  label,
  value,
  saving,
  onCommit,
}: {
  label: string
  value: string
  saving: boolean
  onCommit: (value: string) => void
}) {
  const [draft, setDraft] = useState(value)
  const [editing, setEditing] = useState(false)

  return (
    <input
      type="text"
      className="border-border bg-surface h-7 w-48 shrink-0 rounded-sm border px-2 text-xs"
      // While not being edited the stored value wins, so a save landing
      // elsewhere is reflected instead of being masked by a stale draft.
      value={editing ? draft : value}
      disabled={saving}
      aria-label={label}
      onFocus={() => {
        setDraft(value)
        setEditing(true)
      }}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        setEditing(false)
        if (draft !== value) onCommit(draft)
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Enter') return
        event.currentTarget.blur()
      }}
    />
  )
}
