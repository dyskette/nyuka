/**
 * Which source a browse screen is showing.
 *
 * `/browse` is reachable with nothing chosen, and picking before anything is
 * shown is a step with one sensible answer. The grid and the detail panel must
 * agree, so both ask here.
 */
export function activeSource(
  chosen: string | undefined,
  installed: readonly { id: string }[],
): string | undefined {
  return chosen ?? installed[0]?.id
}
