import { Trans } from '@lingui/react/macro'
import { createFileRoute } from '@tanstack/react-router'

/**
 * Follows.
 *
 * A real route rather than a link to nowhere: the sidebar is part of the
 * shell, so every destination in it has to resolve. The screen is not built
 * yet and says so, which is a different thing from a 404.
 */
export const Route = createFileRoute('/follows')({
  component: FollowsScreen,
})

function FollowsScreen() {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <p className="text-muted-foreground text-sm">
        <Trans>Follows is not built yet.</Trans>
      </p>
    </div>
  )
}
