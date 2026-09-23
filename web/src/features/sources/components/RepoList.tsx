import { Trans, useLingui } from '@lingui/react/macro'
import { useState } from 'react'
import type { components } from '@/shared/api/schema'
import { relativeTime } from '@/shared/lib/time'
import { SourceSettingsPanel } from './SourceSettingsPanel'

type SourceRepo = components['schemas']['SourceRepoDto']
type SourceEntry = components['schemas']['SourceEntryDto']

export interface RepoListProps {
  repos: SourceRepo[]
  /** What each repository offers, by repo id. Absent while still loading. */
  available: ReadonlyMap<string, SourceEntry[]>
  busy?: ReadonlySet<string>
  onRefresh: (repoId: string) => void
  onDelete: (repoId: string) => void
  onInstall: (repoId: string, externalId: string) => void
  onUninstall: (sourceId: string) => void
  /** Installed source ids by `repo_id:external_id`, for the uninstall action. */
  installed: ReadonlyMap<string, string>
}

/**
 * Source repositories and what each one offers.
 *
 * Grouped by repository rather than a flat list of sources: installing means
 * choosing from a repository's index, and uninstalling is undoing that. A flat
 * list would hide where a source came from, which is the thing an operator
 * needs when one of them starts failing.
 */
export function RepoList({
  repos,
  available,
  busy,
  onRefresh,
  onDelete,
  onInstall,
  onUninstall,
  installed,
}: RepoListProps) {
  if (repos.length === 0) return <NoRepos />

  return (
    <ul className="flex flex-col gap-4">
      {repos.map((repo) => (
        <li key={repo.id}>
          <Repo
            repo={repo}
            entries={available.get(repo.id)}
            busy={busy?.has(repo.id) ?? false}
            onRefresh={onRefresh}
            onDelete={onDelete}
            onInstall={onInstall}
            onUninstall={onUninstall}
            installed={installed}
          />
        </li>
      ))}
    </ul>
  )
}

function Repo({
  repo,
  entries,
  busy,
  onRefresh,
  onDelete,
  onInstall,
  onUninstall,
  installed,
}: {
  repo: SourceRepo
  entries: SourceEntry[] | undefined
  busy: boolean
  onRefresh: (repoId: string) => void
  onDelete: (repoId: string) => void
  onInstall: (repoId: string, externalId: string) => void
  onUninstall: (sourceId: string) => void
  installed: ReadonlyMap<string, string>
}) {
  const { t } = useLingui()
  const [confirming, setConfirming] = useState(false)

  return (
    <section className="border-border rounded-md border">
      <header className="border-border p-panel flex items-center gap-2 border-b">
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-medium">{repo.name}</h3>
          <p className="text-muted-foreground truncate text-xs">{repo.url}</p>
        </div>

        <span className="text-muted-foreground shrink-0 text-xs">
          {repo.last_refreshed_at !== null && repo.last_refreshed_at !== undefined ? (
            <Trans>Refreshed {relativeTime(repo.last_refreshed_at)}</Trans>
          ) : (
            <Trans>Never refreshed</Trans>
          )}
        </span>

        <button
          type="button"
          onClick={() => onRefresh(repo.id)}
          disabled={busy}
          className="border-border hover:bg-surface-raised shrink-0 rounded-sm border px-2 py-1 text-xs disabled:opacity-50"
        >
          <Trans>Refresh</Trans>
        </button>

        {/*
          Deleting a repository cascades to every source installed from it and
          then to their series, so it asks first. The confirmation is inline
          rather than a dialog: the thing being confirmed is right here, and a
          modal would hide it.
        */}
        {confirming ? (
          <span className="flex shrink-0 items-center gap-1 text-xs">
            <Trans>Remove this and its series?</Trans>
            <button
              type="button"
              onClick={() => onDelete(repo.id)}
              className="text-danger rounded-sm px-2 py-1"
            >
              <Trans>Remove</Trans>
            </button>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              className="text-muted-foreground rounded-sm px-2 py-1"
            >
              <Trans>Keep</Trans>
            </button>
          </span>
        ) : (
          <button
            type="button"
            onClick={() => setConfirming(true)}
            aria-label={t`Remove ${repo.name}`}
            className="text-muted-foreground hover:text-danger shrink-0 rounded-sm px-2 py-1 text-xs"
          >
            <Trans>Remove</Trans>
          </button>
        )}
      </header>

      <SourceTable
        repoId={repo.id}
        entries={entries}
        onInstall={onInstall}
        onUninstall={onUninstall}
        installed={installed}
      />
    </section>
  )
}

function SourceTable({
  repoId,
  entries,
  onInstall,
  onUninstall,
  installed,
}: {
  repoId: string
  entries: SourceEntry[] | undefined
  onInstall: (repoId: string, externalId: string) => void
  onUninstall: (sourceId: string) => void
  installed: ReadonlyMap<string, string>
}) {
  if (entries === undefined) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>Reading the index…</Trans>
      </p>
    )
  }

  if (entries.length === 0) {
    return (
      <p className="text-muted-foreground p-panel text-sm">
        <Trans>This repository lists no sources. Refresh it to read the index again.</Trans>
      </p>
    )
  }

  return (
    <ul className="flex flex-col">
      {entries.map((entry) => (
        <SourceRow
          key={entry.external_id}
          repoId={repoId}
          entry={entry}
          onInstall={onInstall}
          onUninstall={onUninstall}
          installed={installed}
        />
      ))}
    </ul>
  )
}

/**
 * One source in a repository's index.
 *
 * An installed source can be expanded to show its own settings. Collapsed by
 * default and fetched only when opened: a repository listing twenty sources
 * would otherwise make twenty requests to render a list nobody expanded.
 */
function SourceRow({
  repoId,
  entry,
  onInstall,
  onUninstall,
  installed,
}: {
  repoId: string
  entry: SourceEntry
  onInstall: (repoId: string, externalId: string) => void
  onUninstall: (sourceId: string) => void
  installed: ReadonlyMap<string, string>
}) {
  const { t } = useLingui()
  const [open, setOpen] = useState(false)
  const sourceId = installed.get(`${repoId}:${entry.external_id}`)

  return (
    <li className="border-border border-b last:border-b-0">
      <div className="px-cell flex h-row items-center gap-2 text-sm">
        {sourceId === undefined ? (
          // A source that is not installed has no settings to show, so the
          // space stays empty rather than holding a control that does nothing.
          <span className="w-4 shrink-0" />
        ) : (
          <button
            type="button"
            onClick={() => setOpen((previous) => !previous)}
            aria-expanded={open}
            aria-label={t`Settings for ${entry.name}`}
            className="text-muted-foreground hover:text-foreground w-4 shrink-0 text-xs"
          >
            <span aria-hidden="true">{open ? '\u25BE' : '\u25B8'}</span>
          </button>
        )}
        <span className="min-w-0 flex-1 truncate">{entry.name}</span>
        <span className="text-muted-foreground tabular shrink-0 text-xs">v{entry.version}</span>
        <span className="text-muted-foreground shrink-0 text-xs">{entry.languages.join(', ')}</span>
        <EntryAction
          repoId={repoId}
          entry={entry}
          onInstall={onInstall}
          onUninstall={onUninstall}
          installed={installed}
        />
      </div>

      {open && sourceId !== undefined && (
        <div className="bg-surface-raised border-border border-t">
          <SourceSettingsPanel sourceId={sourceId} />
        </div>
      )}
    </li>
  )
}

/**
 * Install, update, or remove — whichever this entry actually admits.
 *
 * `installed_version` is the server's answer to "is this in", and comparing it
 * to `version` is what distinguishes an available update from an up-to-date
 * install. Offering "install" on something already installed would be a button
 * whose only outcome is a conflict.
 */
function EntryAction({
  repoId,
  entry,
  onInstall,
  onUninstall,
  installed,
}: {
  repoId: string
  entry: SourceEntry
  onInstall: (repoId: string, externalId: string) => void
  onUninstall: (sourceId: string) => void
  installed: ReadonlyMap<string, string>
}) {
  const { t } = useLingui()
  const sourceId = installed.get(`${repoId}:${entry.external_id}`)
  const isInstalled = entry.installed_version !== null && entry.installed_version !== undefined

  if (!isInstalled) {
    return (
      <button
        type="button"
        onClick={() => onInstall(repoId, entry.external_id)}
        aria-label={t`Install ${entry.name}`}
        className="text-accent hover:bg-accent-soft shrink-0 rounded-sm px-2 py-0.5 text-xs"
      >
        <Trans>Install</Trans>
      </button>
    )
  }

  const outdated = (entry.installed_version ?? 0) < entry.version

  return (
    <span className="flex shrink-0 items-center gap-1">
      {outdated && (
        <button
          type="button"
          onClick={() => onInstall(repoId, entry.external_id)}
          aria-label={t`Update ${entry.name}`}
          className="text-accent hover:bg-accent-soft rounded-sm px-2 py-0.5 text-xs"
        >
          <Trans>Update</Trans>
        </button>
      )}
      {sourceId !== undefined && (
        <button
          type="button"
          onClick={() => onUninstall(sourceId)}
          aria-label={t`Uninstall ${entry.name}`}
          className="text-muted-foreground hover:text-danger rounded-sm px-2 py-0.5 text-xs"
        >
          <Trans>Uninstall</Trans>
        </button>
      )}
    </span>
  )
}

function NoRepos() {
  return (
    <div className="flex flex-col items-center gap-2 p-12 text-center">
      <p className="text-sm font-medium">
        <Trans>No repositories</Trans>
      </p>
      <p className="text-muted-foreground max-w-sm text-sm">
        <Trans>
          A repository is a list of sources this server can install from. Add one above.
        </Trans>
      </p>
    </div>
  )
}
