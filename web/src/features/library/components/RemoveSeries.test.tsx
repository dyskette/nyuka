import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '@/test/render'
import { RemoveSeries } from './RemoveSeries'

function props(overrides: Partial<Parameters<typeof RemoveSeries>[0]> = {}) {
  return { title: 'Ashfall Chronicle', onRemove: vi.fn(), ...overrides }
}

describe('RemoveSeries', () => {
  it('asks before removing anything', async () => {
    const onRemove = vi.fn()
    await renderWithProviders(<RemoveSeries {...props({ onRemove })} />)

    await userEvent.click(
      screen.getByRole('button', { name: 'Remove Ashfall Chronicle from your library' }),
    )
    expect(onRemove).not.toHaveBeenCalled()
    expect(screen.getByText(/Remove Ashfall Chronicle from your library\?/)).toBeInTheDocument()
  })

  /** The row comes back by adding the series again; the files do not. */
  it('keeps the files unless they are asked for', async () => {
    const onRemove = vi.fn()
    await renderWithProviders(<RemoveSeries {...props({ onRemove })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    await userEvent.click(screen.getByRole('button', { name: 'Remove' }))

    expect(onRemove).toHaveBeenCalledWith(false)
  })

  it('deletes the files when they are', async () => {
    const onRemove = vi.fn()
    await renderWithProviders(<RemoveSeries {...props({ onRemove })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    await userEvent.click(screen.getByRole('checkbox', { name: /Also delete/ }))
    await userEvent.click(screen.getByRole('button', { name: 'Remove' }))

    expect(onRemove).toHaveBeenCalledWith(true)
  })

  it('backs out without removing anything', async () => {
    const onRemove = vi.fn()
    await renderWithProviders(<RemoveSeries {...props({ onRemove })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }))

    expect(onRemove).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: /Remove Ashfall/ })).toBeInTheDocument()
  })

  /**
   * The one that matters. A reader who ticks the box, thinks better of it and
   * cancels must not find it still ticked next time — the second confirmation
   * would then delete their files on a press they read as the cautious one.
   */
  it('forgets the file choice after a cancel', async () => {
    const onRemove = vi.fn()
    await renderWithProviders(<RemoveSeries {...props({ onRemove })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    await userEvent.click(screen.getByRole('checkbox', { name: /Also delete/ }))
    await userEvent.click(screen.getByRole('button', { name: 'Keep' }))

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    expect(screen.getByRole('checkbox', { name: /Also delete/ })).not.toBeChecked()

    await userEvent.click(screen.getByRole('button', { name: 'Remove' }))
    expect(onRemove).toHaveBeenCalledWith(false)
  })

  it('cannot be pressed twice while it is running', async () => {
    await renderWithProviders(<RemoveSeries {...props({ removing: true })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    expect(screen.getByRole('button', { name: /Removing/ })).toBeDisabled()
  })

  it('reports a failure where the action is', async () => {
    await renderWithProviders(<RemoveSeries {...props({ error: 'the volume is read-only' })} />)

    await userEvent.click(screen.getByRole('button', { name: /Remove Ashfall/ }))
    expect(screen.getByRole('alert')).toHaveTextContent('the volume is read-only')
  })
})
