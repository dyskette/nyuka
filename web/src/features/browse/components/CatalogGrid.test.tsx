import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { CatalogGrid } from './CatalogGrid'

type CatalogItem = components['schemas']['CatalogItemDto']

function item(overrides: Partial<CatalogItem> = {}): CatalogItem {
  return {
    external_key: 'glass-orchard',
    title: 'Glass Orchard',
    authors: [],
    artists: [],
    tags: [],
    status: 'ongoing',
    content_rating: 'safe',
    reading_direction: 'right_to_left',
    ...overrides,
  }
}

describe('CatalogGrid', () => {
  /**
   * A catalog entry has no local identity until it is added, so the only
   * thing to do with a new one is add it. `manga_id` is the server's answer
   * to "is this already in", and it is what decides.
   */
  it('offers to add an entry that is not in the library', async () => {
    const onAdd = vi.fn()
    await renderWithProviders(<CatalogGrid items={[item()]} onAdd={onAdd} />)

    await userEvent.click(screen.getByRole('button', { name: 'Add Glass Orchard to your library' }))
    expect(onAdd).toHaveBeenCalledWith('glass-orchard')
  })

  it('opens an entry that is already in the library instead of adding it again', async () => {
    await renderWithProviders(
      <CatalogGrid
        items={[item({ manga_id: '11111111-1111-4111-8111-111111111111' })]}
        onAdd={vi.fn()}
      />,
    )

    expect(screen.queryByRole('button', { name: /Add/ })).not.toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Open in library' }).getAttribute('href')).toContain(
      '/library/11111111-1111-4111-8111-111111111111',
    )
    // The mockup's "✓ Library" mark. The word carries it, not the glyph.
    expect(screen.getByText('Library')).toBeInTheDocument()
  })

  it('disables the button for an entry already being added', async () => {
    await renderWithProviders(
      <CatalogGrid items={[item()]} adding={new Set(['glass-orchard'])} onAdd={vi.fn()} />,
    )

    expect(screen.getByRole('button', { name: /Add Glass Orchard/ })).toBeDisabled()
  })

  /**
   * Cover hosts are third parties. Without this they learn the address of
   * every server displaying their images.
   */
  it('does not leak the referrer to a source cover host', async () => {
    await renderWithProviders(
      <CatalogGrid
        items={[item({ cover_url: 'https://cdn.example.org/a.jpg' })]}
        onAdd={vi.fn()}
      />,
    )

    expect(document.querySelector('img')).toHaveAttribute('referrerpolicy', 'no-referrer')
  })

  /**
   * The title is rendered beside the cover, so alt text repeating it makes a
   * screen reader announce the series twice.
   */
  it('leaves the cover decorative', async () => {
    await renderWithProviders(
      <CatalogGrid
        items={[item({ cover_url: 'https://cdn.example.org/a.jpg' })]}
        onAdd={vi.fn()}
      />,
    )

    // Empty alt gives the element role `presentation`, so it is absent from
    // the accessibility tree entirely — which is what "decorative" means.
    expect(document.querySelector('img')).toHaveAttribute('alt', '')
    expect(screen.queryByRole('img')).not.toBeInTheDocument()
  })

  /**
   * Adding refetches the page, so an index key would hand one card's in-flight
   * state to whatever moved into its position.
   */
  it('keys cards on the source key, so in-flight state follows the entry', async () => {
    const { rerender } = await renderWithProviders(
      <CatalogGrid
        items={[
          item({ external_key: 'a', title: 'Alpha' }),
          item({ external_key: 'b', title: 'Beta' }),
        ]}
        adding={new Set(['b'])}
        onAdd={vi.fn()}
      />,
    )
    expect(screen.getByRole('button', { name: /Add Beta/ })).toBeDisabled()

    // Alpha leaves the page; Beta is still being added.
    rerender(
      <CatalogGrid
        items={[item({ external_key: 'b', title: 'Beta' })]}
        adding={new Set(['b'])}
        onAdd={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: /Add Beta/ })).toBeDisabled()
  })

  it('says a source returned nothing rather than rendering an empty grid', async () => {
    await renderWithProviders(<CatalogGrid items={[]} onAdd={vi.fn()} />)

    expect(screen.queryByRole('list')).not.toBeInTheDocument()
    expect(screen.getByText(/Nothing here/)).toBeInTheDocument()
  })

  /**
   * The panel is where a reader decides whether to add. The card's own link
   * opens it; the action beside it stays separate, so a click never has to be
   * interpreted as one or the other.
   */
  it('opens the detail panel from the card itself', async () => {
    await renderWithProviders(<CatalogGrid items={[item()]} onAdd={vi.fn()} />)

    expect(screen.getByRole('link', { name: /Glass Orchard/ }).getAttribute('href')).toContain(
      '/browse/glass-orchard',
    )
  })

  it('renders one card per entry', async () => {
    await renderWithProviders(
      <CatalogGrid
        items={[
          item({ external_key: 'a', title: 'Alpha' }),
          item({ external_key: 'b', title: 'Beta' }),
        ]}
        onAdd={vi.fn()}
      />,
    )

    const cards = screen.getAllByRole('listitem')
    expect(cards).toHaveLength(2)
    expect(within(cards[0] as HTMLElement).getByText('Alpha')).toBeInTheDocument()
  })
})
