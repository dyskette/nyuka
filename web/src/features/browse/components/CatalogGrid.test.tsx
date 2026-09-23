import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { CatalogGrid } from './CatalogGrid'

type Manga = components['schemas']['MangaDto']

function manga(overrides: Partial<Manga> = {}): Manga {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    source_id: '22222222-2222-4222-8222-222222222222',
    external_key: 'series-1',
    title: 'Test Series',
    authors: [],
    artists: [],
    tags: [],
    status: 'ongoing',
    content_rating: 'safe',
    reading_direction: 'right_to_left',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-02T00:00:00Z',
    ...overrides,
  }
}

describe('CatalogGrid', () => {
  it('renders one link per series', async () => {
    await renderWithProviders(
      <CatalogGrid
        items={[
          manga({ id: '11111111-1111-4111-8111-111111111111', title: 'Alpha' }),
          manga({ id: '33333333-3333-4333-8333-333333333333', title: 'Beta' }),
        ]}
      />,
    )

    const links = screen.getAllByRole('link')
    expect(links).toHaveLength(2)
    expect(links[0]?.getAttribute('href')).toBe('/library/11111111-1111-4111-8111-111111111111')
  })

  /**
   * A card is a link, not a clickable div. That is what makes it openable in
   * a new tab and announceable by a screen reader, and it is easy to lose in
   * a refactor toward an onClick handler.
   */
  it('makes each card a real link', async () => {
    await renderWithProviders(<CatalogGrid items={[manga({ title: 'Alpha' })]} />)

    const link = screen.getByRole('link', { name: /Alpha/ })
    expect(link.tagName).toBe('A')
    expect(link.getAttribute('href')).toBeTruthy()
  })

  /**
   * The title is rendered next to the cover, so alt text repeating it would
   * make a screen reader announce the series twice.
   */
  it('leaves the cover image decorative', async () => {
    await renderWithProviders(
      <CatalogGrid items={[manga({ cover_url: 'https://example.test/c.jpg' })]} />,
    )

    const image = document.querySelector('img')
    expect(image).not.toBeNull()
    expect(image?.getAttribute('alt')).toBe('')
  })

  it('shows a placeholder when a series has no cover', async () => {
    await renderWithProviders(<CatalogGrid items={[manga({ title: 'Alpha' })]} />)

    const link = screen.getByRole('link', { name: /Alpha/ })
    expect(within(link).getByText('No cover')).toBeTruthy()
    expect(document.querySelector('img')).toBeNull()
  })

  it('lists authors when there are any, and omits the line when there are none', async () => {
    const { unmount } = await renderWithProviders(
      <CatalogGrid items={[manga({ authors: ['Ada', 'Grace'] })]} />,
    )
    expect(screen.getByText('Ada, Grace')).toBeTruthy()
    unmount()

    await renderWithProviders(<CatalogGrid items={[manga({ authors: [] })]} />)
    expect(screen.queryByText(/,/)).toBeNull()
  })

  /**
   * An empty library on a fresh install is the expected state, not an error,
   * and it is the one moment where the next action is genuinely unobvious.
   */
  it('says what to do next when the library is empty', async () => {
    await renderWithProviders(<CatalogGrid items={[]} />)

    expect(screen.getByText('Your library is empty')).toBeTruthy()
    expect(screen.getByText(/Add a source/)).toBeTruthy()
    expect(screen.queryAllByRole('link')).toHaveLength(0)
  })
})
