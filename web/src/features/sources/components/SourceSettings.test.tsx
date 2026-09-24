import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { renderWithProviders } from '@/test/render'
import type { Value } from '../lib/postcard'
import { parseDeclaration } from '../lib/settings'
import { SourceSettings } from './SourceSettings'

function groups(declaration: unknown) {
  return parseDeclaration(declaration)
}

describe('SourceSettings', () => {
  it('renders a switch from its declaration', async () => {
    const onChange = vi.fn()
    await renderWithProviders(
      <SourceSettings
        groups={groups([{ type: 'switch', key: 'locked', title: 'Show Locked Chapters' }])}
        values={new Map()}
        onChange={onChange}
      />,
    )

    const box = screen.getByRole('checkbox', { name: 'Show Locked Chapters' })
    expect(box).not.toBeChecked()

    await userEvent.click(box)
    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ key: 'locked', valueType: 'bool' }),
      true,
    )
  })

  it('shows the stored value as current', async () => {
    await renderWithProviders(
      <SourceSettings
        groups={groups([{ type: 'switch', key: 'locked', title: 'Locked' }])}
        values={new Map<string, Value>([['locked', true]])}
        onChange={vi.fn()}
      />,
    )

    expect(screen.getByRole('checkbox', { name: 'Locked' })).toBeChecked()
  })

  /**
   * `options` are labels and `values` are what the source stores. Writing the
   * label would store something it does not recognise.
   */
  it('writes the stored value, not the label', async () => {
    const onChange = vi.fn()
    await renderWithProviders(
      <SourceSettings
        groups={groups([
          {
            type: 'select',
            key: 'status',
            title: 'Status',
            options: ['Any', 'Ongoing'],
            values: ['', 'ongoing'],
          },
        ])}
        values={new Map()}
        onChange={onChange}
      />,
    )

    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Status' }), 'ongoing')
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ key: 'status' }), 'ongoing')
  })

  /**
   * Preselecting the first option would claim a value the source has not
   * stored, and leave no way back to "unset".
   */
  it('shows an unset select as blank rather than as its first option', async () => {
    await renderWithProviders(
      <SourceSettings
        groups={groups([
          { type: 'select', key: 's', title: 'S', options: ['Any', 'Ongoing'], values: ['a', 'b'] },
        ])}
        values={new Map()}
        onChange={vi.fn()}
      />,
    )

    const select = screen.getByRole('combobox', { name: 'S' }) as HTMLSelectElement
    expect(select.value).toBe('')
  })

  describe('multi-select', () => {
    const declaration = [
      {
        type: 'multi-select',
        key: 'langs',
        title: 'Languages',
        options: ['English', 'Spanish', 'French'],
        values: ['en', 'es', 'fr'],
      },
    ]

    it('reports the whole list, not the one toggled', async () => {
      const onChange = vi.fn()
      await renderWithProviders(
        <SourceSettings
          groups={groups(declaration)}
          values={new Map<string, Value>([['langs', ['en']]])}
          onChange={onChange}
        />,
      )

      await userEvent.click(screen.getByRole('checkbox', { name: 'French' }))
      expect(onChange).toHaveBeenCalledWith(expect.anything(), ['en', 'fr'])
    })

    /**
     * Ordered by the declaration rather than by when each box was ticked, so
     * the same selection always produces the same bytes — otherwise a save
     * writes a different value for a state the reader did not change.
     */
    it('orders by the declaration, not by click order', async () => {
      const onChange = vi.fn()
      await renderWithProviders(
        <SourceSettings
          groups={groups(declaration)}
          values={new Map<string, Value>([['langs', ['fr']]])}
          onChange={onChange}
        />,
      )

      await userEvent.click(screen.getByRole('checkbox', { name: 'English' }))
      expect(onChange).toHaveBeenCalledWith(expect.anything(), ['en', 'fr'])
    })
  })

  /**
   * A text setting saves on blur, not per keystroke: each save is a request
   * and a write into the source's store, and typing a token should not be
   * thirty of them.
   */
  describe('text', () => {
    const declaration = [{ type: 'text', key: 'token', title: 'API token' }]

    it('does not save while typing', async () => {
      const onChange = vi.fn()
      await renderWithProviders(
        <SourceSettings groups={groups(declaration)} values={new Map()} onChange={onChange} />,
      )

      await userEvent.type(screen.getByRole('textbox', { name: 'API token' }), 'abc')
      expect(onChange).not.toHaveBeenCalled()
    })

    it('saves on blur', async () => {
      const onChange = vi.fn()
      await renderWithProviders(
        <SourceSettings groups={groups(declaration)} values={new Map()} onChange={onChange} />,
      )

      await userEvent.type(screen.getByRole('textbox', { name: 'API token' }), 'abc')
      await userEvent.tab()
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ key: 'token' }), 'abc')
    })

    it('does not save a value that did not change', async () => {
      const onChange = vi.fn()
      await renderWithProviders(
        <SourceSettings
          groups={groups(declaration)}
          values={new Map<string, Value>([['token', 'abc']])}
          onChange={onChange}
        />,
      )

      await userEvent.click(screen.getByRole('textbox', { name: 'API token' }))
      await userEvent.tab()
      expect(onChange).not.toHaveBeenCalled()
    })
  })

  /**
   * ADR-0004 lists the webview imports as a declared gap, so a `login`
   * setting cannot work here. Naming it beats hiding it: a reader looking for
   * a setting they know the source has should be told it exists and is not
   * available.
   */
  it('names a control it cannot offer instead of dropping it', async () => {
    await renderWithProviders(
      <SourceSettings
        groups={groups([{ type: 'login', key: 'login', title: 'LOGIN', method: 'web' }])}
        values={new Map()}
        onChange={vi.fn()}
      />,
    )

    const row = screen.getByRole('listitem')
    expect(within(row).getByText('LOGIN')).toBeInTheDocument()
    expect(within(row).getByText(/Not supported yet \(login\)/)).toBeInTheDocument()
  })

  it('locks a control while its save is in flight', async () => {
    await renderWithProviders(
      <SourceSettings
        groups={groups([{ type: 'switch', key: 'locked', title: 'Locked' }])}
        values={new Map()}
        saving={new Set(['locked'])}
        onChange={vi.fn()}
      />,
    )

    expect(screen.getByRole('checkbox', { name: 'Locked' })).toBeDisabled()
  })

  it('says when a source declares nothing', async () => {
    await renderWithProviders(<SourceSettings groups={[]} values={new Map()} onChange={vi.fn()} />)

    expect(screen.getByText(/no settings/)).toBeInTheDocument()
  })
})
