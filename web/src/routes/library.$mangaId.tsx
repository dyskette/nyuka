import { createFileRoute } from '@tanstack/react-router'

/**
 * The detail panel (ADR-0017).
 *
 * Being a route is the point: `loader`, `pendingComponent`, and
 * `errorComponent` come from the router instead of being hand-rolled inside a
 * component, which is what a `?selected=` search parameter would have required.
 */
export const Route = createFileRoute('/library/$mangaId')({
  // TODO(scaffold): loader calls queryClient.ensureQueryData for the manga
  // detail. Loaders prime the cache and return nothing components read —
  // components always read through hooks (ADR-0008).
  component: MangaPanel,
  pendingComponent: PanelSkeleton,
  errorComponent: PanelError,
})

function MangaPanel() {
  const { mangaId } = Route.useParams()
  return <div className="p-4 text-sm">manga: {mangaId}</div>
}

function PanelSkeleton() {
  // Appears after a 150ms delay so fast loads never flash (spec-design.md).
  return <div className="p-4 text-sm opacity-50">loading…</div>
}

function PanelError() {
  return <div className="p-4 text-sm">failed to load</div>
}
