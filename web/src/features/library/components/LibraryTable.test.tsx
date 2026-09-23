import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { LibraryTable, type LibraryTableProps } from './LibraryTable'

type MangaSummary = components['schemas']['MangaSummaryDto']

function summary(overrides: Partial<MangaSummary> = {}): MangaSummary {
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
    source_name: 'Example Source',
    chapter_count: 40,
    downloaded_count: 12,
    ...overrides,
  }
}

function props(overrides: Partial<LibraryTableProps> = {}): LibraryTableProps {
  return {
    items: [summary()],
    sort: 'updated',
    dir: 'desc',
    selected: new Set(),
    onToggle: vi.fn(),
    onToggleAll: vi.fn(),
    ...overrides,
  }
}

describe('LibraryTable', () => {
  /**
   * The whole reason the library is a table: the counts are the content. A
   * regression that dropped them would still render a plausible-looking list.
   */
  it('shows the source and the download counts', async () => {
    await renderWithProviders(<LibraryTable {...props()} />)

    const row = screen.getByRole('row', { name: /Test Series/ })
    expect(within(row).getByText('Example Source')).toBeInTheDocument()
    expect(within(row).getByText('40')).toBeInTheDocument()
    expect(within(row).getByText('12/40')).toBeInTheDocument()
  })

  /**
   * A series whose chapters have not been listed yet has no denominator.
   * `0 / 0` is 1 under any naive fraction, and a full green bar is the one
   * reading that is actively wrong.
   */
  it('does not report an unfetched series as fully downloaded', async () => {
    await renderWithProviders(
      <LibraryTable {...props({ items: [summary({ chapter_count: 0, downloaded_count: 0 })] })} />,
    )

    expect(screen.getByText('0/0')).toBeInTheDocument()
    const bar = document.querySelector('[style*="width"]')
    expect(bar).toHaveStyle({ width: '0%' })
    expect(bar?.className).not.toContain('bg-success')
  })

  describe('sorting', () => {
    it('marks only the sorted column', async () => {
      await renderWithProviders(<LibraryTable {...props({ sort: 'title', dir: 'asc' })} />)

      expect(screen.getByRole('columnheader', { name: /Title/ })).toHaveAttribute(
        'aria-sort',
        'ascending',
      )
      // Not `none`: an inactive column carries no attribute at all, so a
      // screen reader does not announce sort state on every cell.
      expect(screen.getByRole('columnheader', { name: /Updated/ })).not.toHaveAttribute('aria-sort')
      expect(screen.getByRole('columnheader', { name: /Source/ })).not.toHaveAttribute('aria-sort')
    })

    it('reverses the active column and does not touch the others', async () => {
      await renderWithProviders(<LibraryTable {...props({ sort: 'title', dir: 'asc' })} />)

      const title = screen.getByRole('link', { name: /Title/ })
      expect(title.getAttribute('href')).toContain('dir=desc')
    })

    /**
     * Titles read A–Z and recency reads newest-first. Inheriting the previous
     * column's direction gets one of them backwards, which looks like a
     * broken sort rather than a deliberate default.
     */
    it('starts a new column at its own natural direction', async () => {
      await renderWithProviders(<LibraryTable {...props({ sort: 'updated', dir: 'asc' })} />)

      expect(screen.getByRole('link', { name: /Title/ }).getAttribute('href')).toContain('dir=asc')
      expect(screen.getByRole('link', { name: /Chapters/ }).getAttribute('href')).toContain(
        'dir=desc',
      )
    })

    /** A cursor taken under one ordering does not describe another. */
    it('drops the cursor when the ordering changes', async () => {
      await renderWithProviders(<LibraryTable {...props()} />)

      expect(screen.getByRole('link', { name: /Title/ }).getAttribute('href')).not.toContain(
        'cursor',
      )
    })
  })

  describe('selection', () => {
    it('labels each checkbox with its series', async () => {
      await renderWithProviders(<LibraryTable {...props()} />)

      // Not "select row": a forms list of identical labels is unusable.
      expect(screen.getByRole('checkbox', { name: 'Select Test Series' })).toBeInTheDocument()
    })

    it('reports the toggled id', async () => {
      const onToggle = vi.fn()
      await renderWithProviders(<LibraryTable {...props({ onToggle })} />)

      await userEvent.click(screen.getByRole('checkbox', { name: 'Select Test Series' }))
      expect(onToggle).toHaveBeenCalledWith('11111111-1111-4111-8111-111111111111')
    })

    /**
     * `indeterminate` is a DOM property with no attribute, so it is the one
     * piece of this that JSX cannot express and a refactor can silently drop.
     * Without it a partial selection renders as unchecked.
     */
    it('shows a partial selection as indeterminate', async () => {
      const items = [
        summary({ id: '11111111-1111-4111-8111-111111111111' }),
        summary({ id: '33333333-3333-4333-8333-333333333333', title: 'Other' }),
      ]
      await renderWithProviders(
        <LibraryTable
          {...props({ items, selected: new Set(['11111111-1111-4111-8111-111111111111']) })}
        />,
      )

      const all = screen.getByRole('checkbox', {
        name: 'Select every series on this page',
      }) as HTMLInputElement
      expect(all.indeterminate).toBe(true)
      expect(all.checked).toBe(false)
    })

    it('shows a full selection as checked rather than indeterminate', async () => {
      await renderWithProviders(
        <LibraryTable
          {...props({ selected: new Set(['11111111-1111-4111-8111-111111111111']) })}
        />,
      )

      const all = screen.getByRole('checkbox', {
        name: 'Select every series on this page',
      }) as HTMLInputElement
      expect(all.checked).toBe(true)
      expect(all.indeterminate).toBe(false)
    })
  })

  it('explains an empty library instead of rendering an empty table', async () => {
    await renderWithProviders(<LibraryTable {...props({ items: [] })} />)

    expect(screen.queryByRole('table')).not.toBeInTheDocument()
    expect(screen.getByText(/library is empty/i)).toBeInTheDocument()
  })

  describe('virtualization', () => {
    function many(count: number) {
      return Array.from({ length: count }, (_, index) =>
        summary({ id: `id-${index}`, title: `Series ${index}` }),
      )
    }

    /**
     * The point of the whole thing. Five hundred rows in the DOM is what
     * ADR-0017 chose a virtualizer to avoid, and a virtualizer that renders
     * everything is indistinguishable from none until the library is large.
     */
    it('renders a window, not the whole list', async () => {
      const { container } = await renderWithProviders(
        <LibraryTable {...props({ items: many(500) })} />,
      )

      const rows = container.querySelectorAll('tbody tr[aria-rowindex]')
      expect(rows.length).toBeGreaterThan(0)
      expect(rows.length).toBeLessThan(100)
    })

    /**
     * A screen reader is told the size of the list, not the size of the
     * window. Without these it announces "row 3 of 27" in a library of five
     * hundred, because twenty-seven is all that exists in the DOM.
     */
    it('reports the real size and the absolute position', async () => {
      const { container } = await renderWithProviders(
        <LibraryTable {...props({ items: many(500) })} />,
      )

      expect(screen.getByRole('table')).toHaveAttribute('aria-rowcount', '500')

      // Every rendered row, not just the first: asserting only that row one
      // says "1" passes against an index hard-coded to 1, which is what a
      // window-relative index degenerates to at the top of the list.
      const indices = [...container.querySelectorAll('tbody tr[aria-rowindex]')].map((row) =>
        Number(row.getAttribute('aria-rowindex')),
      )
      expect(indices.length).toBeGreaterThan(3)
      expect(indices).toEqual(indices.map((_, offset) => offset + 1))
    })

    /**
     * The spacers are what keep the scrollbar the right length while most
     * rows are absent. They carry no data, so a screen reader must not walk
     * into them.
     */
    it('hides the spacer rows from the accessibility tree', async () => {
      const { container } = await renderWithProviders(
        <LibraryTable {...props({ items: many(500) })} />,
      )

      const spacers = container.querySelectorAll('tbody tr[aria-hidden="true"]')
      expect(spacers.length).toBeGreaterThan(0)
      for (const spacer of spacers) {
        expect(spacer.querySelector('td')).toHaveAttribute('colspan', '7')
      }
    })
  })

  describe('paging', () => {
    function many(count: number) {
      return Array.from({ length: count }, (_, index) =>
        summary({ id: `id-${index}`, title: `Series ${index}` }),
      )
    }

    it('asks for more once the end is in view', async () => {
      const onReachEnd = vi.fn()
      // Few enough that the last row is inside the rendered window.
      await renderWithProviders(
        <LibraryTable {...props({ items: many(5), onReachEnd, hasMore: true })} />,
      )

      expect(onReachEnd).toHaveBeenCalled()
    })

    /**
     * Without this every scroll event during the request asks again, and a
     * slow page turns into a burst of identical requests.
     */
    it('does not ask again while a page is already loading', async () => {
      const onReachEnd = vi.fn()
      await renderWithProviders(
        <LibraryTable
          {...props({ items: many(5), onReachEnd, hasMore: true, loadingMore: true })}
        />,
      )

      expect(onReachEnd).not.toHaveBeenCalled()
    })

    it('does not ask when the server has nothing more', async () => {
      const onReachEnd = vi.fn()
      await renderWithProviders(
        <LibraryTable {...props({ items: many(5), onReachEnd, hasMore: false })} />,
      )

      expect(onReachEnd).not.toHaveBeenCalled()
    })

    it('says a page is on its way', async () => {
      await renderWithProviders(
        <LibraryTable {...props({ items: many(5), loadingMore: true, hasMore: true })} />,
      )

      expect(screen.getByRole('status')).toHaveTextContent('Loading more')
    })
  })
})
