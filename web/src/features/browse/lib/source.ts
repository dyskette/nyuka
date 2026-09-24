/**
 * Which source a browse screen is showing.
 *
 * `/browse` is reachable from the sidebar with nothing chosen, and asking a
 * reader to pick before showing anything is a step with one sensible answer —
 * so the first installed source stands in. `GET /sources` returns a bare
 * array, bounded by what an operator installed, and its order is the server's.
 *
 * This lives here because two routes need the same answer. The catalog grid
 * resolved it and the detail panel did not, so opening a series from a
 * `/browse` URL that carried no `source` landed on "Pick a source to see this
 * series" — while the grid behind it was showing that very source's catalog.
 * The only way out was to change the picker and change it back, which writes
 * the source into the URL.
 */
export function activeSource(
  chosen: string | undefined,
  installed: readonly { id: string }[],
): string | undefined {
  return chosen ?? installed[0]?.id
}
