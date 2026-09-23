import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { FollowTable, type FollowTableProps } from './FollowTable'

type FollowSummary = components['schemas']['FollowSummaryDto']

const MANGA_ID = '11111111-1111-4111-8111-111111111111'

function follow(overrides: Partial<FollowSummary> = {}): FollowSummary {
  return {
    id: 'f-1',
    manga_id: MANGA_ID,
    check_interval_secs: 6 * 3600,
    auto_download: true,
    created_at: '2026-09-01T00:00:00Z',
    manga_title: 'Ashfall Chronicle',
    source_name: 'MangaHaven',
    missing_count: 3,
    ...overrides,
  }
}

function props(overrides: Partial<FollowTableProps> = {}): FollowTableProps {
  return {
    follows: [follow()],
    onCheckNow: vi.fn(),
    onUnfollow: vi.fn(),
    onIntervalChange: vi.fn(),
    onAutoDownloadChange: vi.fn(),
    ...overrides,
  }
}

describe('FollowTable', () => {
  /** The number the screen exists to show, and the only one a reader acts on. */
  it('shows what each follow is waiting to download', async () => {
    await renderWithProviders(<FollowTable {...props()} />)

    const row = screen.getByRole('row', { name: /Ashfall Chronicle/ })
    expect(within(row).getByText('3')).toBeInTheDocument()
    expect(within(row).getByText('MangaHaven')).toBeInTheDocument()
  })

  it('links a follow to the series it watches', async () => {
    await renderWithProviders(<FollowTable {...props()} />)

    expect(screen.getByRole('link', { name: 'Ashfall Chronicle' }).getAttribute('href')).toContain(
      `/library/${MANGA_ID}`,
    )
  })

  describe('the interval', () => {
    it('reports the chosen value in seconds, keyed on the series', async () => {
      const onIntervalChange = vi.fn()
      await renderWithProviders(<FollowTable {...props({ onIntervalChange })} />)

      await userEvent.selectOptions(
        screen.getByRole('combobox', { name: /Check Ashfall Chronicle every/ }),
        String(24 * 3600),
      )

      // The series, not the follow: a follow is identified by what it watches.
      expect(onIntervalChange).toHaveBeenCalledWith(MANGA_ID, 24 * 3600)
    })

    /**
     * An interval set outside this UI — by an older build, or by hand — must
     * still render. Otherwise the select silently shows the first option, and
     * editing any other field writes that wrong value back.
     */
    it('renders a server value that is not one of the offered options', async () => {
      await renderWithProviders(
        <FollowTable {...props({ follows: [follow({ check_interval_secs: 90 * 60 })] })} />,
      )

      const select = screen.getByRole('combobox', { name: /every/ }) as HTMLSelectElement
      expect(select.value).toBe(String(90 * 60))
    })
  })

  it('reports an auto-download change keyed on the series', async () => {
    const onAutoDownloadChange = vi.fn()
    await renderWithProviders(<FollowTable {...props({ onAutoDownloadChange })} />)

    await userEvent.click(
      screen.getByRole('checkbox', {
        name: 'Download new chapters of Ashfall Chronicle automatically',
      }),
    )
    expect(onAutoDownloadChange).toHaveBeenCalledWith(MANGA_ID, false)
  })

  describe('actions', () => {
    it('checks a follow now, by follow id', async () => {
      const onCheckNow = vi.fn()
      await renderWithProviders(<FollowTable {...props({ onCheckNow })} />)

      await userEvent.click(screen.getByRole('button', { name: 'Check Ashfall Chronicle now' }))
      // The follow, not the series: `check-now` addresses the follow.
      expect(onCheckNow).toHaveBeenCalledWith('f-1')
    })

    it('unfollows by follow id', async () => {
      const onUnfollow = vi.fn()
      await renderWithProviders(<FollowTable {...props({ onUnfollow })} />)

      await userEvent.click(
        screen.getByRole('button', { name: 'Stop following Ashfall Chronicle' }),
      )
      expect(onUnfollow).toHaveBeenCalledWith('f-1')
    })

    /** A second change before the first lands would race the server. */
    it('locks a row while something about it is in flight', async () => {
      await renderWithProviders(<FollowTable {...props({ busy: new Set(['f-1']) })} />)

      expect(screen.getByRole('button', { name: /Check .* now/ })).toBeDisabled()
      expect(screen.getByRole('combobox', { name: /every/ })).toBeDisabled()
      expect(screen.getByRole('checkbox')).toBeDisabled()
    })
  })

  it('says a follow has never been checked rather than showing nothing', async () => {
    await renderWithProviders(<FollowTable {...props()} />)

    expect(screen.getByText('Never')).toBeInTheDocument()
  })

  it('explains what following does when there is nothing to list', async () => {
    await renderWithProviders(<FollowTable {...props({ follows: [] })} />)

    expect(screen.queryByRole('table')).not.toBeInTheDocument()
    expect(screen.getByText(/You follow nothing yet/)).toBeInTheDocument()
  })
})
