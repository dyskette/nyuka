import { Trans } from '@lingui/react/macro'
import { useQuery } from '@tanstack/react-query'
import { problemMessage } from '@/shared/api/problem'
import { usePutSourceSetting } from '../api/mutations'
import { sourceSettingsQuery } from '../api/queries'
import { encode, toBase64, type Value } from '../lib/postcard'
import { decodeValues, parseDeclaration } from '../lib/settings'
import { SourceSettings } from './SourceSettings'

/**
 * One installed source's own settings.
 *
 * Fetched only when opened: a library with a dozen sources would otherwise
 * make a dozen requests to render a list nobody expanded.
 */
export function SourceSettingsPanel({ sourceId }: { sourceId: string }) {
  const { data, isPending, error } = useQuery(sourceSettingsQuery(sourceId))
  const put = usePutSourceSetting(sourceId)

  if (isPending) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>Reading this source's settings…</Trans>
      </p>
    )
  }

  if (error) {
    return (
      <p className="text-danger p-panel text-sm" role="alert">
        {problemMessage(error) ?? <Trans>These settings could not be read.</Trans>}
      </p>
    )
  }

  const groups = parseDeclaration(data.declared)
  const settings = groups.flatMap((group) => group.settings)
  const values = decodeValues(settings, data.values)

  return (
    <div className="p-panel flex flex-col gap-3">
      {put.error !== null && put.error !== undefined && (
        <p className="border-danger text-danger rounded-sm border px-3 py-2 text-sm" role="alert">
          {problemMessage(put.error) ?? <Trans>That setting could not be saved.</Trans>}
        </p>
      )}

      <SourceSettings
        groups={groups}
        values={values}
        saving={new Set(put.isPending && put.variables ? [put.variables.key] : [])}
        // Encoding happens here because this is where the declared type is
        // known. The server stores bytes and does not interpret them.
        onChange={(setting, value: Value) =>
          put.mutate({
            key: setting.key,
            value: toBase64(encode(value, setting.valueType)),
          })
        }
      />
    </div>
  )
}
