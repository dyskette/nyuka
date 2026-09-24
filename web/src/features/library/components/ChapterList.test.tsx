import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import type { components } from '@/shared/api/schema'
import { renderWithProviders } from '@/test/render'
import { ChapterList } from './ChapterList'

type ChapterSummary = components['schemas']['ChapterSummaryDto']

function chapter(overrides: Partial<ChapterSummary> = {}): ChapterSummary {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    manga_id: '22222222-2222-4222-8222-222222222222',
    external_key: 'ch-1',
    title: 'The Ninth Gate',
    number: 1,
    downloaded: false,
    ...overrides,
  }
}

describe('ChapterList', () => {
  /**
   * The server orders newest first and pages in that order. Reversing here as
   * well would put the oldest at the top, which is what this did while the
   * server paged ascending: a reader saw the oldest fifty, backwards, and the
   * rest of the series was unreachable.
   */
  it('renders in the order it was given', async () => {
    await renderWithProviders(
      <ChapterList
        chapters={[
          chapter({ id: 'c', number: 3, title: 'Third' }),
          chapter({ id: 'b', number: 2, title: 'Second' }),
          chapter({ id: 'a', number: 1, title: 'First' }),
        ]}
        onDownload={vi.fn()}
      />,
    )

    const titles = screen
      .getAllByRole('listitem')
      .map((li) => within(li).getByText(/First|Second|Third/).textContent)
    expect(titles).toEqual(['Third', 'Second', 'First'])
  })

  it('offers a download for a chapter that has none', async () => {
    const onDownload = vi.fn()
    await renderWithProviders(<ChapterList chapters={[chapter()]} onDownload={onDownload} />)

    await userEvent.click(screen.getByRole('button', { name: 'Download' }))
    expect(onDownload).toHaveBeenCalledWith('11111111-1111-4111-8111-111111111111')
  })

  /**
   * A disabled button in a list of forty rows is noise. The status answers
   * the question the reader has, and offering an action that cannot run is
   * the specific thing being avoided.
   */
  it('shows a status instead of a button once downloaded', async () => {
    await renderWithProviders(
      <ChapterList chapters={[chapter({ downloaded: true })]} onDownload={vi.fn()} />,
    )

    expect(screen.getByText('Downloaded')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Download' })).not.toBeInTheDocument()
  })

  describe('while a download is under way', () => {
    const ID = '11111111-1111-4111-8111-111111111111'

    /**
     * The server returns 202 — the file does not exist yet. Showing
     * "Downloaded" at that point would be a claim about the library that is
     * not true.
     */
    it('shows a queued chapter as queued, not as downloaded', async () => {
      await renderWithProviders(
        <ChapterList
          chapters={[chapter()]}
          downloading={new Map([[ID, { state: 'queued' as const }]])}
          onDownload={vi.fn()}
        />,
      )

      expect(screen.getByText('Queued')).toBeInTheDocument()
      expect(screen.queryByText('Downloaded')).not.toBeInTheDocument()
      expect(screen.queryByRole('button', { name: 'Download' })).not.toBeInTheDocument()
    })

    /**
     * The gap this closes: a chapter went from "Queued" back to a Download
     * button the moment the request returned, and stayed there for the whole
     * download. Nothing on the screen said it was happening.
     */
    it('counts the pages once the first event arrives', async () => {
      await renderWithProviders(
        <ChapterList
          chapters={[chapter()]}
          downloading={new Map([[ID, { state: 'running' as const, done: 12, total: 40 }]])}
          onDownload={vi.fn()}
        />,
      )

      expect(screen.getByText(/Downloading 12\/40/)).toBeInTheDocument()
    })

    /** Progress is not persisted, so a job can be running and not yet be
        anywhere in particular — which is not the same as being queued. */
    it('says downloading without a count when no event has arrived', async () => {
      await renderWithProviders(
        <ChapterList
          chapters={[chapter()]}
          downloading={new Map([[ID, { state: 'running' as const }]])}
          onDownload={vi.fn()}
        />,
      )

      expect(screen.getByText('Downloading')).toBeInTheDocument()
      expect(screen.queryByText('Queued')).not.toBeInTheDocument()
    })

    /** A finished file outranks a stale job row. */
    it('prefers the file over a job still listed as running', async () => {
      await renderWithProviders(
        <ChapterList
          chapters={[chapter({ downloaded: true })]}
          downloading={new Map([[ID, { state: 'running' as const }]])}
          onDownload={vi.fn()}
        />,
      )

      expect(screen.getByText('Downloaded')).toBeInTheDocument()
      expect(screen.queryByText(/Downloading/)).not.toBeInTheDocument()
    })
  })

  /** A decimal chapter is real — 10.5 is not a rounding error. */
  it('keeps a decimal chapter number', async () => {
    await renderWithProviders(
      <ChapterList chapters={[chapter({ number: 10.5 })]} onDownload={vi.fn()} />,
    )

    expect(screen.getByText('10.5')).toBeInTheDocument()
  })

  /**
   * Sources commonly key a chapter by its number, and the number already has
   * a column — a row reading "50  50" looks like a rendering fault.
   */
  it('does not print the number twice when the key is the number', async () => {
    await renderWithProviders(
      <ChapterList
        chapters={[chapter({ title: null, number: 50, external_key: '50' })]}
        onDownload={vi.fn()}
      />,
    )

    expect(screen.getAllByText('50')).toHaveLength(1)
  })

  it('falls back to the source key when a chapter has no title', async () => {
    await renderWithProviders(
      <ChapterList
        chapters={[chapter({ title: null, external_key: 'vol-2-ch-7' })]}
        onDownload={vi.fn()}
      />,
    )

    expect(screen.getByText('vol-2-ch-7')).toBeInTheDocument()
  })

  it('explains an empty chapter list', async () => {
    await renderWithProviders(<ChapterList chapters={[]} onDownload={vi.fn()} />)

    expect(screen.queryByRole('list')).not.toBeInTheDocument()
    expect(screen.getByText(/No chapters\./)).toBeInTheDocument()
  })

  describe('retrieving the file', () => {
    /**
     * Downloading a chapter to the server is only half of it. Without this
     * there is no way to get one onto the device the reader is holding.
     */
    it('offers a downloaded chapter as a link to its archive', async () => {
      await renderWithProviders(
        <ChapterList chapters={[chapter({ downloaded: true })]} onDownload={vi.fn()} />,
      )

      const save = screen.getByRole('link', { name: /Save The Ninth Gate/ })
      expect(save).toHaveAttribute(
        'href',
        '/api/v1/downloads/11111111-1111-4111-8111-111111111111/file',
      )
    })

    /**
     * No `download` attribute. The server sends `Content-Disposition` with the
     * archive's real name, which carries the series, volume and chapter in the
     * shape other readers parse — setting one here would override it with
     * whatever this page happened to know.
     */
    it('leaves the filename to the server', async () => {
      await renderWithProviders(
        <ChapterList chapters={[chapter({ downloaded: true })]} onDownload={vi.fn()} />,
      )

      expect(screen.getByRole('link', { name: /Save/ })).not.toHaveAttribute('download')
    })

    it('offers nothing to save for a chapter with no file', async () => {
      await renderWithProviders(<ChapterList chapters={[chapter()]} onDownload={vi.fn()} />)

      expect(screen.queryByRole('link', { name: /Save/ })).not.toBeInTheDocument()
    })
  })
})
