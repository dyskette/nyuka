import { Trans, useLingui } from '@lingui/react/macro'
import { useQueries, useSuspenseQuery } from '@tanstack/react-query'
import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import {
  useAddRepo,
  useDeleteRepo,
  useInstallSource,
  useRefreshRepo,
  useUninstallSource,
} from '@/features/sources/api/mutations'
import {
  availableSourcesQuery,
  repoListQuery,
  sourceListQuery,
} from '@/features/sources/api/queries'
import { RepoList } from '@/features/sources/components/RepoList'
import { problemMessage } from '@/shared/api/problem'

/**
 * Settings — for now, where sources come from.
 *
 * Repositories and their sources rather than a flat list: installing is
 * choosing from a repository's index, and where a source came from is what an
 * operator needs when one starts failing.
 */
export const Route = createFileRoute('/settings')({
  loader: ({ context }) =>
    Promise.all([
      context.queryClient.ensureQueryData(repoListQuery()),
      context.queryClient.ensureQueryData(sourceListQuery()),
    ]),

  component: SettingsScreen,
  errorComponent: SettingsError,
})

function SettingsScreen() {
  const { data: repos } = useSuspenseQuery(repoListQuery())
  const { data: sources } = useSuspenseQuery(sourceListQuery())

  // One query per repository, which `useQueries` runs in parallel. A single
  // endpoint returning every repository's index would be one request, but the
  // server does not offer one, and issuing them serially would make the page
  // as slow as the sum of every repository.
  const availableQueries = useQueries({
    queries: repos.map((repo) => availableSourcesQuery(repo.id)),
  })

  const available = new Map(
    repos.flatMap((repo, index) => {
      const entries = availableQueries[index]?.data
      return entries === undefined ? [] : [[repo.id, entries] as const]
    }),
  )

  // `repo_id:external_id` is the only thing identifying an entry across the
  // two endpoints: the available list has no local source id, and the
  // installed list has no repository entry.
  const installed = new Map(
    sources.map((source) => [`${source.repo_id}:${source.external_id}`, source.id] as const),
  )

  const install = useInstallSource()
  const uninstall = useUninstallSource()
  const refresh = useRefreshRepo()
  const remove = useDeleteRepo()

  const busy = new Set(refresh.isPending && refresh.variables ? [refresh.variables] : [])

  return (
    /*
     * The scroll container is the full width; the content is centred inside
     * it. With `overflow-y-auto` on the centred box itself, the scrollbar was
     * drawn at that box's right edge — in the middle of the window, with
     * empty page either side of it.
     */
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-4 p-6">
        <h1 className="text-lg font-semibold">
          <Trans>Sources</Trans>
        </h1>

        <AddRepoForm />

        {/* Every mutation's failure is surfaced here rather than swallowed: a
          silent no-op after pressing Install is the worst of the options. */}
        <MutationError error={install.error ?? uninstall.error ?? refresh.error ?? remove.error} />

        <RepoList
          repos={repos}
          available={available}
          busy={busy}
          installed={installed}
          onRefresh={(id) => refresh.mutate(id)}
          onDelete={(id) => remove.mutate(id)}
          onInstall={(repoId, externalId) => install.mutate({ repoId, externalId })}
          onUninstall={(id) => uninstall.mutate(id)}
        />
      </div>
    </div>
  )
}

function AddRepoForm() {
  const { t } = useLingui()
  const add = useAddRepo()
  const [url, setUrl] = useState('')
  const [name, setName] = useState('')

  return (
    <form
      className="border-border flex items-end gap-2 rounded-md border p-3"
      onSubmit={(event) => {
        event.preventDefault()
        if (url.trim() === '' || name.trim() === '') return
        add.mutate(
          { url: url.trim(), name: name.trim() },
          // Cleared only on success: a rejected URL stays in the field so it
          // can be corrected rather than retyped.
          {
            onSuccess: () => {
              setUrl('')
              setName('')
            },
          },
        )
      }}
    >
      <label className="flex flex-1 flex-col gap-1 text-xs">
        <Trans>Repository URL</Trans>
        <input
          type="url"
          required
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://example.org/index.min.json"
          aria-label={t`Repository URL`}
          className="border-border bg-surface h-8 rounded-sm border px-2 text-sm"
        />
      </label>

      <label className="flex w-48 flex-col gap-1 text-xs">
        <Trans>Name</Trans>
        <input
          required
          value={name}
          onChange={(event) => setName(event.target.value)}
          aria-label={t`Repository name`}
          className="border-border bg-surface h-8 rounded-sm border px-2 text-sm"
        />
      </label>

      <button
        type="submit"
        disabled={add.isPending}
        className="border-border hover:bg-surface-raised h-8 shrink-0 rounded-sm border px-3 text-sm disabled:opacity-50"
      >
        {add.isPending ? <Trans>Adding…</Trans> : <Trans>Add repository</Trans>}
      </button>
    </form>
  )
}

/**
 * The last failed mutation, in words.
 *
 * problem+json carries a `detail` written for a person (RFC 9457), so the
 * server's own explanation is shown rather than a generic message that throws
 * away what it said.
 */
function MutationError({ error }: { error: unknown }) {
  if (error === null || error === undefined) return null

  return (
    <p
      // `role="alert"` so it is announced: the control that caused it may be
      // far from here, and a message nobody hears is not a message.
      role="alert"
      className="border-danger text-danger rounded-sm border px-3 py-2 text-sm"
    >
      {/* `problemMessage` returns null when the server said nothing specific,
          which would render an empty alert box. */}
      {problemMessage(error) ?? <Trans>That did not work.</Trans>}
    </p>
  )
}

function SettingsError() {
  return (
    <div className="p-8 text-sm">
      <Trans>Settings could not be loaded.</Trans>
    </div>
  )
}
