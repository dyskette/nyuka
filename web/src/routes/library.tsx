import { createFileRoute, Outlet, retainSearchParams } from '@tanstack/react-router'
import { z } from 'zod'

/**
 * The library master list, and the parent of the detail panel (ADR-0017).
 *
 * Search parameters are validated here, on the parent, so the detail route
 * inherits them and `retainSearchParams` can preserve them across opening and
 * closing the panel.
 */
const searchSchema = z.object({
  q: z.string().optional(),
  sort: z.enum(['title', 'updated', 'added']).default('updated'),
  dir: z.enum(['asc', 'desc']).default('desc'),
  cursor: z.string().optional(),
})

export const Route = createFileRoute('/library')({
  // Zod 4 implements Standard Schema, so the schema is passed directly.
  // Do NOT add @tanstack/zod-adapter — it does not work with Zod 4 (ADR-0008).
  validateSearch: searchSchema,

  // Without this, navigating to /library/$mangaId drops these from the URL,
  // the parent re-defaults them, and the user's filtered view silently resets
  // — while the list component stays mounted, so it reads as a data bug rather
  // than a routing one (ADR-0017).
  search: { middlewares: [retainSearchParams(['q', 'sort', 'dir', 'cursor'])] },

  component: LibraryLayout,
})

function LibraryLayout() {
  const search = Route.useSearch()

  return (
    <div className="flex h-dvh">
      {/* TODO(scaffold): DataTable — TanStack Table + Virtual. */}
      <main className="flex-1 overflow-hidden">
        <p className="p-4 text-sm">
          library: sort={search.sort} dir={search.dir}
        </p>
      </main>
      {/* The detail panel renders here. The parent stays mounted, so the
          virtualizer's scroll offset and the infinite query's accumulated
          pages survive opening it. */}
      <aside className="w-[420px] shrink-0 border-l">
        <Outlet />
      </aside>
    </div>
  )
}
