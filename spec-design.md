## Design system & UX spec — v1

### Principles

1. Information first: every pixel of chrome must justify itself against a table row.
2. State is shown where it happens: rows change in place; the status bar aggregates; toasts only carry news from elsewhere.
3. Motion explains, never decorates: something moves only when it helps the eye track a change.
4. Keyboard parity: any action reachable by mouse has a shortcut and appears in ⌘K.
5. Both themes are designed, not derived: tokens are authored per theme, not inverted.

### Tokens (Tailwind v4 `@theme`, shadcn-compatible)

Color (OKLCH, neutral scale with a slight cool cast so grays don't look muddy in dark mode):

| Token | Light | Dark | Use |
|---|---|---|---|
| `--background` | `oklch(99% 0 0)` | `oklch(14% 0.005 260)` | page |
| `--surface` | `oklch(100% 0 0)` | `oklch(17% 0.005 260)` | panels, table body |
| `--surface-raised` | `oklch(97.5% 0 0)` | `oklch(20% 0.005 260)` | hover rows, popovers |
| `--border` | `oklch(92% 0 0)` | `oklch(26% 0.005 260)` | hairlines |
| `--border-strong` | `oklch(84% 0 0)` | `oklch(34% 0.005 260)` | dividers that must read. **Not** the focus indicator: at 1.63:1 on white it misses the 3:1 required of a state indicator, so focus is carried by `--ring` (ADR-0016) |
| `--foreground` | `oklch(20% 0 0)` | `oklch(93% 0 0)` | primary text |
| `--muted-foreground` | `oklch(50% 0 0)` | `oklch(65% 0 0)` | secondary text |
| `--accent` | `oklch(55% 0.18 262)` | `oklch(70% 0.16 262)` | one hue (indigo-blue); selection, primary button, active nav, progress |
| `--accent-foreground` | white | `oklch(14% 0.005 260)` | |
| `--accent-soft` | `oklch(95% 0.03 262)` | `oklch(25% 0.06 262)` | selected row bg |
| `--success` `--warning` `--danger` | teal / amber / red at 55% L — **except `--success`, which needs 53% L** (at 55% it measures 4.46:1 on white and fails 4.5:1) | same hues at 70% L | status dots and text only, never fills |
| `--ring` | **opaque `--accent`** (at 60% alpha it measures 1.92:1 over a light surface and fails the 3:1 required of a focus indicator; opaque gives 5.00:1 — ADR-0016) | same | focus ring |

Rule: semantic colors appear as 6px dots, text, or 2px bars, never as filled surfaces — this is what keeps a single-accent palette readable when there are many job states.

Two caveats on this table (ADR-0016): OKLCH lightness is not WCAG luminance, so equal `L` across hues gives unequal contrast — `success`, `warning` and `danger` at 55% L measure 4.46, 4.95 and 5.38 against white. And five tokens fall outside sRGB (light `--accent-soft`, `--success`, `--warning`; dark `--accent`, `--danger`), so browsers gamut-map them and the rendered colour is not the specified one. Verify contrast against rendered output, never against token values.

Typography: `--font-sans: Inter Variable` (self-hosted, `font-display: swap`, subsetted latin + latin-ext), `--font-mono: Geist Mono` or `JetBrains Mono` for IDs, sizes, timestamps, chapter numbers. Scale (compact / comfortable):

| Role | Size / line | Weight |
|---|---|---|
| Page title | 18 / 24 | 600 |
| Section / panel title | 14 / 20 | 600 |
| Body & table cell | 13 / 20 (14 / 20 comfortable) | 400 |
| Meta, captions, mono values | 12 / 16 | 400, mono `tabular-nums` |
| Badge | 11 / 16 | 500, `tracking-wide` |

Numbers in tables always `font-variant-numeric: tabular-nums` so live counters don't jitter.

Spacing: 4px base; density toggle switches a data attribute `data-density="compact|comfortable"` that remaps `--row-h` (32/40), `--cell-px` (8/12), `--panel-p` (12/16). Nothing else changes with density.

Shape & elevation: `--radius-sm 4px` (inputs, badges, rows), `--radius 6px` (cards, popovers, dialogs), no larger. Elevation is expressed by border + background step, not shadow; popovers and dialogs get one `shadow-sm` (`0 1px 2px rgb(0 0 0 / .06)` light, `0 1px 2px rgb(0 0 0 / .4)` dark) plus border.

### Layout

Three-column app shell on a 1280+ desktop:

```
┌ sidebar 220px ┬ master list (flex) ┬ detail panel 420–560px (resizable) ┐
│ nav           │ toolbar 40px       │ header: cover + title + actions    │
│               │ table (virtual)    │ tabs: Overview · Chapters · Jobs   │
│               │                    │                                     │
├───────────────┴────────────────────┴─────────────────────────────────────┤
│ status bar 28px: ● 3 downloading 1.2 MB/s · 14 queued · live            │
└──────────────────────────────────────────────────────────────────────────┘
```

- Sidebar: Library, Browse, Downloads, Follows, Settings; collapses to 48px icon rail at ≤1024 (persisted), becomes a `Sheet` at ≤768. Active item: `--accent-soft` bg + 2px left accent bar.
- Master list toolbar: search input (focus with `/`), filter chips, sort menu, view toggle (list/grid in Browse only), selection count and bulk actions appear in place of the filters when ≥1 row is selected.
- Detail panel: opens on row select as a nested route `/library/$mangaId`, with tabs as child routes so each gets its own loader and boundaries; the parent list route keeps its filters via `retainSearchParams` (ADR-0017). `Esc` navigates to the parent, `[`/`]` resize presets. Below 1024 the panel takes the full content area with a back button.
- Status bar: left = SSE state dot (accent = live, amber = reconnecting, red = offline) + active downloads summary; center = aggregate progress hairline (2px) spanning the bar; right = queue count, disk free (mono). Click opens Downloads.

### Components (shadcn base, customized)

- `DataTable`: sticky header, 32px rows, hover `--surface-raised`, selected `--accent-soft`, focused row 1px inset ring; columns resizable, reorderable, visibility in a `⋯` menu; right-aligned mono for numbers; row height animates 150ms on density change.
- `ProgressCell`: 2px bar under the row content (not a chunky bar), percent + `12.4 / 48.0 MB` mono, `transition: width 300ms linear` with values interpolated between SSE ticks so it never jumps; active row gets a 2s opacity pulse on its status dot only (`@keyframes pulse` on `--accent`).
- `StatusDot` + text: `queued` gray, `running` accent (pulsing), `succeeded` success, `failed` danger, `cancelled` muted strikethrough text.
- `Cover`: 3:4 ratio, `object-fit: cover`, 4px radius, `--surface-raised` placeholder with blurred low-res (from backend thumbhash) → full image fade 200ms; row thumbnail 24×32, panel cover 160×213, grid card 140×187.
- `CommandPalette` (cmdk): sections Actions, Navigate, Sources, Library, Recent; shows shortcut hints on the right in mono.
- `Kbd` component for all shortcut hints; `Badge` monochrome by default, accent for "new chapters" counts.
- `DynamicSettingsForm` for Aidoku source settings: label left (200px), control right, help text muted 12px; grouped by the source's manifest sections.
- `EmptyState`: 20px monochrome lucide glyph, one sentence, one primary button, optional secondary link; max width 360px, vertically centered in the pane.
- `Skeleton`: same row height and column widths as the table it replaces; shimmer replaced by a 1.2s opacity breathe (cheaper, calmer); appears after 150ms delay so fast loads never flash.

### Motion spec

| Case | Duration / easing |
|---|---|
| Hover, focus, color, opacity | 120ms `ease-out` |
| Detail panel open/close | 200ms `cubic-bezier(.2,.8,.2,1)` translateX 8px + fade (no full slide-in) |
| Popover, menu, dialog | 150ms scale .98→1 + fade; exit 100ms |
| Row insert/remove (virtual list) | none; new rows just appear — animation fights virtualization |
| Progress width | 300ms linear |
| Sidebar collapse | 200ms width, labels fade 120ms |
| Toast | 200ms in from bottom-right 8px, exit 150ms; stack max 3 |
| Route change | none; content replaces, skeleton after 150ms |
| Reduced motion | all durations 0 except progress (kept, it's information); pulse replaced by static accent dot |

Implemented with **`tw-animate-css`** utilities (`tailwindcss-animate` is deprecated and has no Tailwind v4 support) plus `@starting-style` and `transition-behavior: allow-discrete` for enter/exit, and CSS variables `--dur-fast: 120ms`, `--dur: 200ms`, `--ease-out`, all wrapped in `@media (prefers-reduced-motion: reduce)`. No JavaScript animation library (ADR-0018).

### Keyboard map

Global: `⌘K` palette · `g l / g b / g d / g f / g ,` navigate · `/` focus search · `?` shortcut sheet · `Esc` close panel or clear selection.
Lists: `j/k` or `↑/↓` move · `x` toggle select · `⇧↑/↓` extend · `Enter` open detail · `o` open in new pane · `Space` preview cover.
Manga/chapters: `d` download selected · `⇧D` schedule · `f` follow/unfollow · `r` refresh metadata.
Jobs: `c` cancel · `⇧R` retry · `,`/`.` filter by state.
Every shortcut is registered in one map (`shared/lib/shortcuts.ts`) that feeds both the key handler and the palette hints, so they can't drift.

### States & feedback

- Mutations: optimistic row update; on failure the row reverts and shows a danger `StatusDot` + inline `Retry`; a toast appears only if the affected row is not visible.
- Errors from problem+json: title as headline, detail as body, field errors under inputs; 401 redirects; 429 shows a countdown from `Retry-After` on the disabled button.
- SSE offline: status bar dot amber, banner only after 30s ("Live updates paused, retrying…"), never blocks the UI.
- Long tasks (installing a source, packaging): inline progress in the row, no modal.
- Destructive actions (delete downloaded files, remove source): `AlertDialog` with the count and the object name, confirm button labelled with the verb, `Enter` does not confirm — `⌘Enter` does.

### Accessibility (WCAG 2.2 AA)

- Contrast ≥ 4.5:1 text, ≥ 3:1 UI; checked in CI with `axe-core` in Vitest and Playwright.
- Visible focus everywhere: 2px `--ring` outline offset 2px; never `outline: none` without replacement. Focus not obscured by sticky header/status bar (2.4.11).
- Tables: `role="grid"` with roving tabindex, `aria-sort`, `aria-selected`, `aria-rowcount` for virtual lists, column headers as buttons.
- Live regions: status bar summary `aria-live="polite"` throttled to one announcement per 10s; failures `aria-live="assertive"`.
- Target size ≥ 24×24 even in compact (2.5.8); density toggle affects text/rows, not hit areas.
- Icons never carry meaning alone: status dots paired with text or `aria-label`; color never the only signal.
- Drag (column reorder, panel resize) has a keyboard/menu equivalent (2.5.7).
- Reduced motion, forced-colors (`@media (forced-colors: active)` keeps borders and focus), 200% zoom layout tested.
- i18n: strings never concatenated, plurals via Lingui ICU, `lang` attribute per locale, layout tolerant of ~30% longer strings; RTL not in scope (declared).

### Theming mechanics

`<html data-theme="light|dark">` set before first paint by an inline script reading `localStorage` then `prefers-color-scheme`; a `theme-color` meta per theme; `color-scheme` set on `:root` so scrollbars and form controls match; images in dark mode get no dimming (covers must stay accurate).

### Deliverables from this section

- `web/src/app/theme.css` with the token table above.
- ADRs in [`docs/adr/`](docs/adr/README.md): [0016](docs/adr/0016-use-a-single-accent-neutral-color-system.md) single-accent neutral system (with the four required token corrections), [0017](docs/adr/0017-address-the-detail-panel-with-a-nested-route.md) master–detail as a nested route, [0018](docs/adr/0018-keep-motion-in-css.md) CSS-only motion.
- Component inventory checklist mapped to shadcn primitives to install.
