import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { http, server } from '@/test/server'
import { CommandPalette } from './CommandPalette'

type MangaSummary = components['schemas']['MangaSummaryDto']

function summary(overrides: Partial<MangaSummary> = {}): MangaSummary {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    source_id: '22222222-2222-4222-8222-222222222222',
    external_key: 'ashfall',
    title: 'Ashfall Chronicle',
    authors: [],
    artists: [],
    tags: [],
    status: 'ongoing',
    content_rating: 'safe',
    reading_direction: 'right_to_left',
    created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-20T00:00:00Z',
    source_name: 'MangaHaven',
    chapter_count: 86,
    downloaded_count: 72,
    ...overrides,
  }
}

/** What `GET /manga` returns while the palette is open. */
function stubLibrary(items: MangaSummary[]) {
  server.use(http.get('/manga', ({ response }) => response(200).json({ items })))
}

beforeEach(() => stubLibrary([summary()]))

describe('CommandPalette', () => {
  it('renders nothing while closed', async () => {
    await renderWithProviders(<CommandPalette open={false} onOpenChange={vi.fn()} />)

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
  })

  it('offers every screen to jump to', async () => {
    await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

    const group = screen.getByText('Go to').closest('[cmdk-group]')
    const labels = within(group as HTMLElement)
      .getAllByRole('option')
      .map((o) => o.textContent)
    expect(labels).toEqual(['Library', 'Browse', 'Downloads', 'Follows', 'Settings'])
  })

  /**
   * The four kinds `POST /jobs` accepts, and the only UI they have anywhere.
   * `refresh_metadata` is deliberately absent: the mockup shows it, the server
   * does not accept it, and an entry that cannot run is worse than a missing
   * one.
   */
  it('offers exactly the maintenance jobs the server accepts', async () => {
    await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

    const group = screen.getByText('Maintenance').closest('[cmdk-group]')
    const labels = within(group as HTMLElement)
      .getAllByRole('option')
      .map((o) => o.textContent)

    expect(labels).toEqual([
      'Update sources',
      'Reconcile library',
      'Prune finished jobs',
      'Prune expired sessions',
    ])
    expect(screen.queryByText(/Refresh metadata/)).not.toBeInTheDocument()
  })

  it('narrows to what was typed', async () => {
    await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

    await userEvent.type(screen.getByRole('combobox'), 'prune')

    expect(screen.getByText('Prune finished jobs')).toBeInTheDocument()
    expect(screen.queryByText('Update sources')).not.toBeInTheDocument()
    // A navigation entry that does not match is gone too.
    expect(screen.queryByText('Downloads')).not.toBeInTheDocument()
  })

  /**
   * Searching a source is what a reader does when the library does not have
   * the thing, so it is offered precisely when nothing else matched.
   */
  it('offers a source search once something is typed', async () => {
    await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

    expect(screen.queryByText(/in Browse/)).not.toBeInTheDocument()

    await userEvent.type(screen.getByRole('combobox'), 'zebra')
    expect(screen.getByText(/Search .*zebra.* in Browse/)).toBeInTheDocument()
  })

  it('closes when an entry is chosen', async () => {
    const onOpenChange = vi.fn()
    await renderWithProviders(<CommandPalette open onOpenChange={onOpenChange} />)

    await userEvent.click(screen.getByText('Settings'))
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  describe('the library', () => {
    it('lists matching series with their counts', async () => {
      await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

      const option = await screen.findByRole('option', { name: /Ashfall Chronicle/ })
      expect(option).toHaveTextContent('MangaHaven')
      // Downloaded over total. Not "14 new" as the mockup shows — nothing
      // records what has been read.
      expect(option).toHaveTextContent('72/86')
    })

    /**
     * The mockup shows actions belonging to whichever series is highlighted.
     * They are rendered unfiltered, which is why `cmdk`'s own filtering is
     * off — typing a title must not hide the actions for it.
     */
    it('shows actions for the highlighted series', async () => {
      await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

      await waitFor(() =>
        expect(screen.getByText('Actions on Ashfall Chronicle')).toBeInTheDocument(),
      )
      expect(screen.getByRole('option', { name: 'Open chapters' })).toBeInTheDocument()
      expect(screen.getByRole('option', { name: 'Follow this series' })).toBeInTheDocument()
      expect(screen.getByRole('option', { name: 'Browse MangaHaven' })).toBeInTheDocument()
    })

    /**
     * Two sources can carry the same title. Keying the highlight on the title
     * would show one series' actions while the other was selected.
     */
    it('keys a series on its id, not its title', async () => {
      stubLibrary([
        summary({ id: 'a', source_name: 'MangaHaven' }),
        summary({ id: 'b', source_name: 'Kaizoku' }),
      ])
      await renderWithProviders(<CommandPalette open onOpenChange={vi.fn()} />)

      const options = await screen.findAllByRole('option', { name: /Ashfall Chronicle/ })
      expect(options).toHaveLength(2)
      expect(options.map((o) => o.getAttribute('data-value'))).toEqual(['manga:a', 'manga:b'])
    })
  })
})
