import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { RepoList, type RepoListProps } from './RepoList'

type SourceRepo = components['schemas']['SourceRepoDto']
type SourceEntry = components['schemas']['SourceEntryDto']

const REPO_ID = '11111111-1111-4111-8111-111111111111'

function repo(overrides: Partial<SourceRepo> = {}): SourceRepo {
  return {
    id: REPO_ID,
    name: 'Example Repo',
    url: 'https://example.org/index.min.json',
    created_at: '2026-09-01T00:00:00Z',
    ...overrides,
  }
}

function entry(overrides: Partial<SourceEntry> = {}): SourceEntry {
  return {
    repo_id: REPO_ID,
    external_id: 'mangahaven',
    name: 'MangaHaven',
    version: 2,
    languages: ['en'],
    content_rating: 'safe',
    ...overrides,
  }
}

function props(overrides: Partial<RepoListProps> = {}): RepoListProps {
  return {
    repos: [repo()],
    available: new Map([[REPO_ID, [entry()]]]),
    installed: new Map(),
    onRefresh: vi.fn(),
    onDelete: vi.fn(),
    onInstall: vi.fn(),
    onUninstall: vi.fn(),
    ...overrides,
  }
}

describe('RepoList', () => {
  it('offers to install a source that is not installed', async () => {
    const onInstall = vi.fn()
    await renderWithProviders(<RepoList {...props({ onInstall })} />)

    await userEvent.click(screen.getByRole('button', { name: 'Install MangaHaven' }))
    expect(onInstall).toHaveBeenCalledWith(REPO_ID, 'mangahaven')
  })

  /**
   * `installed_version` is the server's answer to "is this in". Offering
   * install on something already installed is a button whose only outcome is
   * a conflict.
   */
  it('offers uninstall, not install, once a source is in', async () => {
    const onUninstall = vi.fn()
    await renderWithProviders(
      <RepoList
        {...props({
          available: new Map([[REPO_ID, [entry({ installed_version: 2 })]]]),
          installed: new Map([[`${REPO_ID}:mangahaven`, 'source-1']]),
          onUninstall,
        })}
      />,
    )

    expect(screen.queryByRole('button', { name: /^Install/ })).not.toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Uninstall MangaHaven' }))
    expect(onUninstall).toHaveBeenCalledWith('source-1')
  })

  /**
   * An outdated install is the one case that offers two actions. Comparing
   * the versions is what distinguishes it from an up-to-date one, and getting
   * that backwards hides every available update.
   */
  it('offers an update only when the repository has a newer version', async () => {
    await renderWithProviders(
      <RepoList
        {...props({
          available: new Map([[REPO_ID, [entry({ version: 3, installed_version: 2 })]]]),
          installed: new Map([[`${REPO_ID}:mangahaven`, 'source-1']]),
        })}
      />,
    )
    expect(screen.getByRole('button', { name: 'Update MangaHaven' })).toBeInTheDocument()
  })

  it('offers no update when the installed version is current', async () => {
    await renderWithProviders(
      <RepoList
        {...props({
          available: new Map([[REPO_ID, [entry({ version: 2, installed_version: 2 })]]]),
          installed: new Map([[`${REPO_ID}:mangahaven`, 'source-1']]),
        })}
      />,
    )
    expect(screen.queryByRole('button', { name: /Update/ })).not.toBeInTheDocument()
  })

  /**
   * Deleting a repository cascades to its sources and then to their series,
   * so a single misplaced click must not do it.
   */
  it('asks before removing a repository', async () => {
    const onDelete = vi.fn()
    await renderWithProviders(<RepoList {...props({ onDelete })} />)

    await userEvent.click(screen.getByRole('button', { name: 'Remove Example Repo' }))
    expect(onDelete).not.toHaveBeenCalled()

    expect(screen.getByText(/Remove this and its series\?/)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Remove' }))
    expect(onDelete).toHaveBeenCalledWith(REPO_ID)
  })

  it('backs out of the confirmation without removing anything', async () => {
    const onDelete = vi.fn()
    await renderWithProviders(<RepoList {...props({ onDelete })} />)

    await userEvent.click(screen.getByRole('button', { name: 'Remove Example Repo' }))
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }))

    expect(onDelete).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Remove Example Repo' })).toBeInTheDocument()
  })

  it('refreshes a repository', async () => {
    const onRefresh = vi.fn()
    await renderWithProviders(<RepoList {...props({ onRefresh })} />)

    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }))
    expect(onRefresh).toHaveBeenCalledWith(REPO_ID)
  })

  /**
   * A repository whose index has not been read yet is a different state from
   * one that lists nothing — the first resolves on its own, the second needs
   * a refresh.
   */
  it('distinguishes a loading index from an empty one', async () => {
    const { rerender } = await renderWithProviders(
      <RepoList {...props({ available: new Map() })} />,
    )
    expect(screen.getByText(/Reading the index/)).toBeInTheDocument()

    rerender(<RepoList {...props({ available: new Map([[REPO_ID, []]]) })} />)
    expect(screen.getByText(/lists no sources/)).toBeInTheDocument()
  })

  it('says what a repository is when there are none', async () => {
    await renderWithProviders(<RepoList {...props({ repos: [] })} />)

    expect(screen.getByText(/No repositories/)).toBeInTheDocument()
  })

  it('says when a repository has never been refreshed', async () => {
    await renderWithProviders(<RepoList {...props()} />)

    const header = screen.getByRole('heading', { name: 'Example Repo' }).closest('header')
    expect(within(header as HTMLElement).getByText(/Never refreshed/)).toBeInTheDocument()
  })
})
