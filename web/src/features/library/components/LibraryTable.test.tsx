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
})
