import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { openRowLink } from './rowClick'

function Row({ onNavigate, onSelect }: { onNavigate: () => void; onSelect: () => void }) {
  return (
    <table>
      <tbody>
        <tr onClick={openRowLink}>
          <td>
            <input type="checkbox" aria-label="Select" onChange={onSelect} />
          </td>
          <td>
            <a
              href="/somewhere"
              data-row-link=""
              onClick={(event) => {
                event.preventDefault()
                onNavigate()
              }}
            >
              Ashfall Chronicle
            </a>
          </td>
          <td>
            <button type="button" onClick={onSelect}>
              Cancel
            </button>
          </td>
          <td>elsewhere in the row</td>
        </tr>
      </tbody>
    </table>
  )
}

describe('openRowLink', () => {
  it('opens the row from anywhere that is not a control', async () => {
    const onNavigate = vi.fn()
    render(<Row onNavigate={onNavigate} onSelect={vi.fn()} />)

    await userEvent.click(screen.getByText('elsewhere in the row'))
    expect(onNavigate).toHaveBeenCalled()
  })

  /** A checkbox in a row selects it. Opening the series as well would make
      selecting one impossible. */
  it('leaves a checkbox to do its own job', async () => {
    const onNavigate = vi.fn()
    const onSelect = vi.fn()
    render(<Row onNavigate={onNavigate} onSelect={onSelect} />)

    await userEvent.click(screen.getByRole('checkbox'))
    expect(onSelect).toHaveBeenCalled()
    expect(onNavigate).not.toHaveBeenCalled()
  })

  it('leaves a button alone', async () => {
    const onNavigate = vi.fn()
    render(<Row onNavigate={onNavigate} onSelect={vi.fn()} />)

    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(onNavigate).not.toHaveBeenCalled()
  })

  /** Clicking the link itself must open it once, not twice. */
  it('does not double-fire on the link itself', async () => {
    const onNavigate = vi.fn()
    render(<Row onNavigate={onNavigate} onSelect={vi.fn()} />)

    await userEvent.click(screen.getByRole('link'))
    expect(onNavigate).toHaveBeenCalledTimes(1)
  })
})
