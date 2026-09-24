import { Trans, useLingui } from '@lingui/react/macro'
import { useState } from 'react'

export interface RemoveSeriesProps {
  title: string
  /** True while the request is in flight. */
  removing?: boolean
  /** The server's explanation, if the last attempt failed. */
  error?: string | undefined
  onRemove: (files: boolean) => void
}

/**
 * Removing a series, and deciding what happens to what was downloaded of it.
 *
 * Two destructive acts with different costs: the library row comes back by
 * adding the series again, and the files come back only by downloading them
 * again. So they are two decisions, and the second is off unless asked for.
 *
 * Inline rather than a dialog, as the repository list does it: the thing being
 * confirmed is on screen, and a modal would cover it.
 */
export function RemoveSeries({ title, removing = false, error, onRemove }: RemoveSeriesProps) {
  const { t } = useLingui()
  const [confirming, setConfirming] = useState(false)
  const [files, setFiles] = useState(false)

  function cancel() {
    setConfirming(false)
    // Reset, so reopening the confirmation never arrives with the destructive
    // half already ticked from a previous time.
    setFiles(false)
  }

  if (!confirming) {
    return (
      <button
        type="button"
        onClick={() => setConfirming(true)}
        aria-label={t`Remove ${title} from your library`}
        className="text-muted-foreground hover:text-danger rounded-sm px-2 py-1 text-xs"
      >
        <Trans>Remove from library</Trans>
      </button>
    )
  }

  return (
    <div className="border-border flex flex-col gap-2 rounded-sm border p-2 text-xs">
      <p>
        <Trans>Remove {title} from your library?</Trans>
      </p>

      <label className="flex items-center gap-1.5">
        <input
          type="checkbox"
          checked={files}
          onChange={(event) => setFiles(event.target.checked)}
        />
        <Trans>Also delete the downloaded files</Trans>
      </label>

      <div className="flex items-center gap-1">
        <button
          type="button"
          onClick={() => onRemove(files)}
          disabled={removing}
          className="text-danger rounded-sm px-2 py-1 disabled:opacity-50"
        >
          {removing ? <Trans>Removing…</Trans> : <Trans>Remove</Trans>}
        </button>
        <button
          type="button"
          onClick={cancel}
          disabled={removing}
          className="text-muted-foreground rounded-sm px-2 py-1 disabled:opacity-50"
        >
          <Trans>Keep</Trans>
        </button>
      </div>

      {error !== undefined && (
        <p className="text-danger" role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
