import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '@/test/render'
import { type FilterState, parseFilters } from '../lib/filters'
import { CatalogFilters } from './CatalogFilters'

const ASURA = [
  { type: 'sort', id: 'sort', title: 'Sort', canAscend: true, options: ['Latest', 'Popular'] },
  {
    type: 'select',
    id: 'status',
    title: 'Status',
    options: ['All', 'Ongoing'],
    ids: ['', 'ongoing'],
  },
  {
    type: 'multi-select',
    id: 'genres',
    title: 'Genres',
    canExclude: true,
    options: ['Action', 'Comedy'],
  },
]

function open(state: FilterState = {}, onChange = vi.fn()) {
  return { filters: parseFilters(ASURA), state, onChange }
}

describe('CatalogFilters', () => {
  /** Collapsed by default: a source can declare a dozen. */
  it('hides the controls until asked, unless something is set', async () => {
    await renderWithProviders(<CatalogFilters {...open()} />)
    expect(screen.queryByRole('combobox', { name: 'Status' })).not.toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: /Filters/ }))
    expect(screen.getByRole('combobox', { name: 'Status' })).toBeInTheDocument()
  })

  /** A filtered view must never look unfiltered. */
  it('opens already expanded when a filter is set', async () => {
    await renderWithProviders(
      <CatalogFilters {...open({ status: { Select: { id: 'status', value: 'ongoing' } } })} />,
    )

    expect(screen.getByRole('combobox', { name: 'Status' })).toBeInTheDocument()
    expect(screen.getByText('1')).toBeInTheDocument()
  })

  it('sends the id the source matches on, not the label', async () => {
    const onChange = vi.fn()
    await renderWithProviders(<CatalogFilters {...open({}, onChange)} />)
    await userEvent.click(screen.getByRole('button', { name: /Filters/ }))

    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'ongoing')
    expect(onChange).toHaveBeenCalledWith({
      status: { Select: { id: 'status', value: 'ongoing' } },
    })
  })

  describe('multi-select', () => {
    /**
     * Three states on one control, because a checkbox has two and a source
     * that allows exclusion needs a third.
     */
    it('cycles a genre from off to included to excluded', async () => {
      const onChange = vi.fn()
      const { rerender } = await renderWithProviders(<CatalogFilters {...open({}, onChange)} />)
      await userEvent.click(screen.getByRole('button', { name: /Filters/ }))

      await userEvent.click(screen.getByRole('button', { name: 'Action: not filtered' }))
      expect(onChange).toHaveBeenLastCalledWith({
        genres: { MultiSelect: { id: 'genres', included: ['Action'], excluded: [] } },
      })

      rerender(
        <CatalogFilters
          {...open(
            { genres: { MultiSelect: { id: 'genres', included: ['Action'], excluded: [] } } },
            onChange,
          )}
        />,
      )
      await userEvent.click(screen.getByRole('button', { name: 'Action: included' }))
      expect(onChange).toHaveBeenLastCalledWith({
        genres: { MultiSelect: { id: 'genres', included: [], excluded: ['Action'] } },
      })
    })

    /** A source that cannot exclude must not be sent an exclusion. */
    it('skips the excluded state when the source cannot exclude', async () => {
      const onChange = vi.fn()
      const filters = parseFilters([
        { type: 'multi-select', id: 'g', title: 'G', options: ['Action'] },
      ])
      await renderWithProviders(
        <CatalogFilters
          filters={filters}
          state={{ g: { MultiSelect: { id: 'g', included: ['Action'], excluded: [] } } }}
          onChange={onChange}
        />,
      )

      await userEvent.click(screen.getByRole('button', { name: 'Action: included' }))
      expect(onChange).toHaveBeenLastCalledWith({
        g: { MultiSelect: { id: 'g', included: [], excluded: [] } },
      })
    })
  })

  describe('sort', () => {
    /**
     * The direction toggle appears only when the source says the order can be
     * reversed. Offering it otherwise sends a flag the source ignores, and the
     * listing does not change — which reads as a broken control.
     */
    it('offers a direction only when the source can ascend', async () => {
      await renderWithProviders(
        <CatalogFilters
          {...open({ sort: { Sort: { id: 'sort', index: 0, ascending: false } } })}
        />,
      )
      expect(screen.getByRole('button', { name: /Sort ascending/ })).toBeInTheDocument()
    })

    it('offers none when it cannot', async () => {
      const filters = parseFilters([{ type: 'sort', id: 's', title: 'S', options: ['A'] }])
      await renderWithProviders(
        <CatalogFilters
          filters={filters}
          state={{ s: { Sort: { id: 's', index: 0, ascending: false } } }}
          onChange={vi.fn()}
        />,
      )
      expect(screen.queryByRole('button', { name: /Sort / })).not.toBeInTheDocument()
    })
  })

  it('clears every filter at once', async () => {
    const onChange = vi.fn()
    await renderWithProviders(
      <CatalogFilters
        {...open({ status: { Select: { id: 'status', value: 'ongoing' } } }, onChange)}
      />,
    )

    await userEvent.click(screen.getByRole('button', { name: 'Clear' }))
    expect(onChange).toHaveBeenCalledWith({})
  })

  /** The host has no Range value, so a control for it would be refused. */
  it('names a filter type it cannot offer', async () => {
    const filters = parseFilters([{ type: 'range', id: 'year', title: 'Year' }])
    await renderWithProviders(<CatalogFilters filters={filters} state={{}} onChange={vi.fn()} />)
    await userEvent.click(screen.getByRole('button', { name: /Filters/ }))

    expect(screen.getByText(/Year is not available here \(range\)/)).toBeInTheDocument()
  })

  it('renders nothing when a source declares no filters', async () => {
    const { container } = await renderWithProviders(
      <CatalogFilters filters={[]} state={{}} onChange={vi.fn()} />,
    )
    expect(container).toBeEmptyDOMElement()
  })
})
