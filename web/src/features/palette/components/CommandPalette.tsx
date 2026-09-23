import { Trans, useLingui } from '@lingui/react/macro'
import { useMutation, useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Command } from 'cmdk'
import { useCallback, useEffect, useState } from 'react'
import { useUpsertFollow } from '@/features/follows/api/mutations'
import { libraryListQuery } from '@/features/library/api/queries'
import { api } from '@/shared/api/client'
import type { components } from '@/shared/api/schema'
import { rankBy } from '../lib/match'

type MangaSummary = components['schemas']['MangaSummaryDto']

/**
 * The maintenance jobs an operator may queue by hand.
 *
 * Exactly the server's `TRIGGERABLE` list. The other kinds take a payload
 * naming a chapter or a series and each has its own endpoint, so offering
 * them here would be a second, unvalidated way to enqueue the same work.
 *
 * This is also the only place these four have any UI at all.
 */
const MAINTENANCE = [
  { kind: 'update_sources', label: 'Update sources' },
  { kind: 'reconcile_library', label: 'Reconcile library' },
  { kind: 'prune_jobs', label: 'Prune finished jobs' },
  { kind: 'prune_sessions', label: 'Prune expired sessions' },
] as const

const DESTINATIONS = [
  { to: '/library', label: 'Library' },
  { to: '/browse', label: 'Browse' },
  { to: '/downloads', label: 'Downloads' },
  { to: '/follows', label: 'Follows' },
  { to: '/settings', label: 'Settings' },
] as const

export interface CommandPaletteProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

/**
 * The ⌘K palette (from the mockup).
 *
 * Search the library, jump between screens, act on the highlighted series, and
 * queue maintenance. Filtering is done here rather than by `cmdk`
 * (`shouldFilter={false}`) because the actions group has to stay visible
 * whatever is typed, and because result order is meaningful.
 *
 * # The mockup's "Refresh metadata" is absent
 *
 * `POST /jobs` accepts only the four maintenance kinds; `refresh_metadata`
 * takes a payload naming a series and has no endpoint. An entry that cannot
 * run is worse than a missing one.
 *
 * # "Download 14 new chapters" is absent
 *
 * It needs the chapter list for the highlighted series to know which are
 * missing, then one request per chapter. That is a query and a burst of
 * writes behind a single keypress; the panel's chapter list does it per
 * chapter, where the reader can see what they are asking for.
 */
export function CommandPalette({ open, onOpenChange }: CommandPaletteProps) {
  const { t } = useLingui()
  const navigate = useNavigate()
  const [query, setQuery] = useState('')

  // The series whose actions are shown. Tracked separately from `cmdk`'s
  // current value, and updated only when that value names a series: arrowing
  // down *into* the actions group changes the highlight to an action, and
  // deriving the group from the live value would make it vanish the moment it
  // was entered.
  const [activeId, setActiveId] = useState<string | null>(null)

  const onHighlight = useCallback((value: string) => {
    const id = value.startsWith('manga:') ? value.slice('manga:'.length) : null
    if (id !== null) setActiveId(id)
  }, [])

  // Only fetched while the palette is open. Mounting this query at the root
  // would load the library on every screen to serve a panel nobody opened.
  const { data: library } = useQuery({ ...libraryListQuery(), enabled: open })

  const series = rankBy(library?.items ?? [], query, (m) => m.title).slice(0, 8)
  const destinations = rankBy([...DESTINATIONS], query, (d) => d.label)
  const maintenance = rankBy([...MAINTENANCE], query, (m) => m.label)

  // Falls back to the first result, because `cmdk` highlights it on open and
  // the mockup shows the actions group filled from the start.
  const active = series.find((m) => m.id === activeId) ?? series[0]

  const run = useCallback(
    (action: () => void) => {
      // Closed first, so the screen being navigated to is what the reader
      // sees rather than the palette dissolving over it.
      onOpenChange(false)
      setQuery('')
      action()
    },
    [onOpenChange],
  )

  // The query is cleared on close as well as on run: reopening should not
  // resume a search abandoned an hour ago.
  useEffect(() => {
    if (!open) setQuery('')
  }, [open])

  const follow = useUpsertFollow()
  const trigger = useTriggerMaintenance()

  if (!open) return null

  return (
    <Command.Dialog
      open={open}
      onOpenChange={onOpenChange}
      label={t`Command palette`}
      shouldFilter={false}
      // Uncontrolled selection: `cmdk` highlights the first item on open and
      // re-highlights as the list narrows. Driving `value` from state here
      // would leave nothing selected until the first arrow key.
      onValueChange={onHighlight}
      className="bg-background/60 fixed inset-0 z-50 flex items-start justify-center p-4 pt-[12vh] backdrop-blur-sm"
    >
      <div className="bg-surface border-border w-full max-w-xl overflow-hidden rounded-lg border shadow-lg">
        <Command.Input
          value={query}
          onValueChange={setQuery}
          placeholder={t`Search your library, or type a command`}
          className="border-border h-11 w-full border-b bg-transparent px-3 text-sm outline-none"
        />

        <Command.List className="max-h-80 overflow-y-auto p-1">
          <Command.Empty className="text-muted-foreground p-6 text-center text-sm">
            <Trans>Nothing matches that.</Trans>
          </Command.Empty>

          {series.length > 0 && (
            <Group heading={t`Library`}>
              {series.map((manga) => (
                <Item
                  key={manga.id}
                  value={itemValue(manga)}
                  onSelect={() =>
                    run(
                      () =>
                        void navigate({
                          to: '/library/$mangaId',
                          params: { mangaId: manga.id },
                          search: { tab: 'overview' },
                        }),
                    )
                  }
                >
                  <span className="truncate">{manga.title}</span>
                  <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                    {manga.source_name} · {manga.downloaded_count}/{manga.chapter_count}
                  </span>
                </Item>
              ))}
            </Group>
          )}

          {/*
            Contextual, as the mockup shows: the actions belong to whichever
            series is highlighted. Rendered unfiltered, which is the reason
            `cmdk`'s own filtering is off.
          */}
          {active !== undefined && (
            <Group heading={t`Actions on ${active.title}`}>
              <Item
                value={`action:open:${active.id}`}
                onSelect={() =>
                  run(
                    () =>
                      void navigate({
                        to: '/library/$mangaId',
                        params: { mangaId: active.id },
                        search: { tab: 'chapters' },
                      }),
                  )
                }
              >
                <Trans>Open chapters</Trans>
              </Item>
              <Item
                value={`action:follow:${active.id}`}
                onSelect={() => run(() => follow.mutate({ manga_id: active.id }))}
              >
                <Trans>Follow this series</Trans>
              </Item>
              <Item
                value={`action:source:${active.id}`}
                onSelect={() =>
                  run(
                    () =>
                      void navigate({
                        to: '/browse',
                        search: { source: active.source_id },
                      }),
                  )
                }
              >
                <Trans>Browse {active.source_name}</Trans>
              </Item>
            </Group>
          )}

          {destinations.length > 0 && (
            <Group heading={t`Go to`}>
              {destinations.map((destination) => (
                <Item
                  key={destination.to}
                  value={`goto:${destination.to}`}
                  onSelect={() => run(() => void navigate({ to: destination.to }))}
                >
                  {destination.label}
                </Item>
              ))}
            </Group>
          )}

          {/*
            Always offered, even with no match above: searching a source is
            what a reader does when the library does not have the thing.
          */}
          {query.trim() !== '' && (
            <Group heading={t`Search sources`}>
              <Item
                value="search-sources"
                onSelect={() =>
                  run(() => void navigate({ to: '/browse', search: { q: query.trim() } }))
                }
              >
                <Trans>Search “{query.trim()}” in Browse</Trans>
              </Item>
            </Group>
          )}

          {maintenance.length > 0 && (
            <Group heading={t`Maintenance`}>
              {maintenance.map((job) => (
                <Item
                  key={job.kind}
                  value={`job:${job.kind}`}
                  onSelect={() => run(() => trigger.mutate(job.kind))}
                >
                  {job.label}
                </Item>
              ))}
            </Group>
          )}
        </Command.List>

        <footer className="border-border text-muted-foreground flex items-center gap-3 border-t px-3 py-1.5 text-xs">
          <span>
            <Trans>↑↓ navigate</Trans>
          </span>
          <span>
            <Trans>↵ open</Trans>
          </span>
          <span>
            <Trans>esc close</Trans>
          </span>
        </footer>
      </div>
    </Command.Dialog>
  )
}

/**
 * The value `cmdk` identifies an item by, and which `onValueChange` reports.
 *
 * Prefixed and keyed on the id rather than the title: two series can share a
 * title across sources, and highlighting one would then show the other's
 * actions.
 */
function itemValue(manga: MangaSummary): string {
  return `manga:${manga.id}`
}

function Group({ heading, children }: { heading: string; children: React.ReactNode }) {
  return (
    <Command.Group
      heading={heading}
      className="[&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs"
    >
      {children}
    </Command.Group>
  )
}

function Item({
  value,
  onSelect,
  children,
}: {
  value: string
  onSelect: () => void
  children: React.ReactNode
}) {
  return (
    <Command.Item
      value={value}
      onSelect={onSelect}
      className="data-[selected=true]:bg-accent-soft flex h-8 cursor-default items-center gap-2 rounded-sm px-2 text-sm"
    >
      {children}
    </Command.Item>
  )
}

/**
 * Queues one of the four maintenance jobs.
 *
 * Deliberately not invalidating the job list on success: the server answers
 * 202 and the row appears over SSE like any other job. Refetching here would
 * race that for no benefit.
 */
function useTriggerMaintenance() {
  return useMutation({
    mutationFn: async (kind: string) => {
      const { error } = await api.POST('/jobs', { body: { kind } })
      if (error !== undefined) throw error
    },
  })
}
