import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '@/test/render'
import { LibraryToolbar, type LibraryToolbarProps } from './LibraryToolbar'

function props(overrides: Partial<LibraryToolbarProps> = {}): LibraryToolbarProps {
  return {
    filters: { q: '', status: '', source: '' },
    sources: ['Kaizoku', 'MangaHaven'],
    statuses: ['completed', 'ongoing'],
    shown: 12,
    total: 12,
    onChange: vi.fn(),
    ...overrides,
  }
}

describe('LibraryToolbar', () => {
  /**
   * The placeholder vanishes on focus and a screen reader in forms mode may
   * never announce it, so it cannot be the only label.
   */
  it('labels the search box rather than relying on the placeholder', async () => {
    await renderWithProviders(<LibraryToolbar {...props()} />)

    expect(screen.getByRole('searchbox', { name: /Search 12 titles/ })).toBeInTheDocument()
  })

  /**
   * One history entry per keystroke would make the back button delete the
   * query a letter at a time. The debounce is what prevents that, and it is
   * invisible in every other test.
   */
  it('reports the query once the typing stops, not per keystroke', async () => {
    const onChange = vi.fn()
    await renderWithProviders(<LibraryToolbar {...props({ onChange })} />)

    // Real timers rather than fake ones: `renderWithProviders` awaits the
    // router's own load, which never resolves while the clock is frozen.
    await userEvent.type(screen.getByRole('searchbox'), 'ash')
    expect(onChange).not.toHaveBeenCalled()

    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1))
    expect(onChange).toHaveBeenCalledWith({ q: 'ash' })
  })

  /**
   * The URL is the source of truth. If the box kept its own value, the back
   * button would change the results while the search field went on showing
   * the query that no longer applied.
   */
  it('takes a new value from the URL', async () => {
    const { rerender } = await renderWithProviders(<LibraryToolbar {...props()} />)

    rerender(<LibraryToolbar {...props({ filters: { q: 'orchard', status: '', source: '' } })} />)
    expect(screen.getByRole('searchbox')).toHaveValue('orchard')
  })

  it('offers the sources and statuses it was given', async () => {
    await renderWithProviders(<LibraryToolbar {...props()} />)

    const source = screen.getByRole('combobox', { name: /Source/ })
    expect(
      within(source)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['All', 'Kaizoku', 'MangaHaven'])
  })

  /**
   * An absent filter has to leave the URL entirely, so the empty option
   * reports the empty string and the route turns that into a removed key. A
   * sentinel like "all" would be indistinguishable from a source called that.
   */
  it('reports an empty string when a filter is cleared', async () => {
    const onChange = vi.fn()
    await renderWithProviders(
      <LibraryToolbar
        {...props({ filters: { q: '', status: 'ongoing', source: '' }, onChange })}
      />,
    )

    await userEvent.selectOptions(screen.getByRole('combobox', { name: /Status/ }), '')
    expect(onChange).toHaveBeenCalledWith({ status: '' })
  })

  it('says how many rows a filter left', async () => {
    await renderWithProviders(<LibraryToolbar {...props({ shown: 3, total: 12 })} />)

    expect(screen.getByText('3 of 12')).toBeInTheDocument()
  })
})
