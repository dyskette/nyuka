/**
 * Opens a row's own link when the row is clicked.
 *
 * A table row is a click target people expect, but it must not become a second
 * definition of where the row goes: this finds the link already in the row and
 * follows it, so the destination has one source of truth and the keyboard,
 * middle click and "copy link address" keep working through it.
 *
 * A click that lands on a control is left to that control — a checkbox selects
 * the row, it does not open it.
 */
export function openRowLink(event: React.MouseEvent<HTMLElement>): void {
  if (event.defaultPrevented) return

  const target = event.target
  if (!(target instanceof Element)) return
  if (target.closest('a, button, input, select, textarea, label')) return

  event.currentTarget.querySelector<HTMLAnchorElement>('a[data-row-link]')?.click()
}
