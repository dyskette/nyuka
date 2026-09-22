# ADR-0018: Keep motion in CSS

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/app/theme.css`, `web/src/shared/ui` |
| **Supersedes** | None |

This article explains why animation is expressed in CSS transitions and keyframes rather than a JavaScript animation library. It also corrects the package the design specification names, which shadcn has deprecated, and records the two platform features that make CSS-only enter and exit animations viable without JavaScript.

## Context

### The motion budget is deliberately small

The design specification's third principle is that motion explains and never decorates, and its motion table reflects that: 120 ms for hover and focus, 200 ms for the detail panel, 150 ms for popovers, 300 ms linear for progress, and — notably — **none** for row insertion and removal in virtual lists and **none** for route changes.

The single most demanding case is `ProgressCell`, which interpolates width between SSE ticks so the bar never jumps. That is a transition on one property, not an animation system.

### Reduced motion is a hard requirement with an exception

The specification states that under `prefers-reduced-motion: reduce` all durations go to zero **except progress**, because progress is information rather than decoration, and the status-dot pulse is replaced by a static dot. That exception is easy to express as a CSS rule and awkward to thread through a JavaScript animation library's configuration.

### Virtualization rules out list animation anyway

The table is TanStack Virtual over TanStack Table. The specification already concludes that animating row insertion "fights virtualization" — rows are recycled DOM nodes whose content changes, so an enter animation would fire on a node that was never absent. This removes the single most common reason teams reach for an animation library.

### Two platform features changed what CSS can do

Enter and exit animations used to require JavaScript to keep an element mounted while it animated out. Two features fixed that, and both are Baseline:

- **`@starting-style`** defines the state an entry transition starts from.
- **`transition-behavior: allow-discrete`** lets transitions run on discrete properties such as `display`.

Both became Baseline Newly Available in August 2024 and sit at roughly 91% global usage in early 2026. Same-document **View Transitions** also reached Baseline Newly Available once Firefox 144 shipped.

## Decision

Express all motion in CSS. Do not add a JavaScript animation library.

- **Transitions and keyframes only**, driven by the specification's duration and easing tokens: `--dur-fast: 120ms`, `--dur: 200ms`, and the easing variables.
- **Use `tw-animate-css`, not `tailwindcss-animate`.**

  > [!IMPORTANT]
  > The design specification names `tailwindcss-animate`. shadcn has **deprecated** it in favor of `tw-animate-css`, which is the package with Tailwind v4 support and is installed by default in new shadcn projects. Since ADR-0008 records the toolchain as Tailwind v4 on Vite 8, the specification's package reference is stale. Correct it before setup instructions are written from it.

- **Use `@starting-style` and `transition-behavior: allow-discrete`** for popover, menu, dialog, and toast enter and exit. This is what keeps exit animations out of JavaScript.
- **Radix handles state, CSS handles motion.** shadcn components built on Radix expose `data-state` attributes; animate those with CSS selectors rather than driving animation from React state.
- **No motion on virtual list rows and no route transitions**, per the specification. Content replaces, with a skeleton after 150 ms.
- **Reduced motion is one block.** Wrap durations in `@media (prefers-reduced-motion: reduce)` setting them to zero, with progress width explicitly exempt and the pulse keyframe replaced by a static accent dot.
- **Do not adopt View Transitions in v1.** Same-document transitions are Baseline, but the specification calls for no route transition at all, so there is nothing for them to do yet. Cross-document transitions are **not** Baseline — Firefox and Safari do not implement them — so any future plan must not depend on them.

## Consequences

### What you gain

- **No animation code in the bundle.** Given ADR-0013's finding that a 200 KB telemetry SDK was worth deferring, spending comparable weight on decoration would be inconsistent. A motion library is typically tens of kilobytes; CSS is zero.
- **Animation runs off the main thread.** Transitions on `opacity` and `transform` are compositor-driven, which matters because the main thread is handling SSE events, virtual list recalculation, and Query cache updates. A JavaScript animation loop would compete with exactly the work that makes the UI feel live.
- **Reduced motion is genuinely one rule.** The media query overrides the duration variables in a single place, and the progress exemption is a one-line exception rather than a conditional threaded through component props.
- **Motion cannot drift from the design tokens**, because it reads the same CSS variables the rest of the theme does.
- **Nothing to keep compatible.** No animation library tracking React's release cadence, and no interaction with concurrent rendering.
- **Forced-colors mode degrades cleanly.** CSS transitions on color simply have no visible effect when the system overrides colors, with no JavaScript assuming an animation completed.

### What it costs you

- **No FLIP or layout animations.** Animating an element between two layout positions is what a JavaScript library does well and CSS does poorly. The specification does not ask for it, but a future "card expands into panel" effect would be genuinely hard.
- **No gesture-driven motion.** Drag-to-dismiss and velocity-aware springs are out of reach. The panel resize via `[` and `]` is preset-based, which sidesteps this.
- **Interruption is crude.** CSS transitions handle a mid-flight target change acceptably but without spring physics, so rapid state changes can look mechanical. Progress at 300 ms linear is the case to watch, since SSE ticks may arrive faster than the transition completes.
- **`@starting-style` is at roughly 91% support**, not 100%. The remaining browsers get no enter animation, which is a graceful degradation rather than a break — but it should be a conscious acceptance, not a surprise.
- **Orchestration is manual.** Staggered sequences mean hand-written delays. The specification's motion table has no sequences, so this is currently free.

### Follow-up work this decision creates

1. **Replace `tailwindcss-animate` with `tw-animate-css`** in the specification and in setup, so the deprecated package never gets installed.
2. **Test reduced motion**, asserting durations collapse to zero and that **progress width still animates** — the exception is the part most likely to be lost in a refactor.
3. **Verify progress interpolation against real SSE cadence.** If `job.progress` events arrive faster than 300 ms, the transition never completes and the bar may appear to stall. Measure with the actual event rate from ADR-0010 before tuning the duration.
4. **Build popover, dialog, and toast exits with `@starting-style` and `allow-discrete`**, and confirm no component keeps itself mounted in JavaScript to animate out.
5. **Confirm no `data-state` animation is driven from React state**, so Radix remains the state authority.
6. **Check the 150 ms skeleton delay** actually prevents a flash on fast loads, since a delay that is too short produces the flicker it exists to avoid.

## Alternatives considered

| Alternative | Bundle cost | Layout and gesture animation | Reduced motion | Outcome |
|---|---|---|---|---|
| CSS transitions and keyframes | 0 | No | One media query | **Chosen** |
| Motion (formerly Framer Motion) | Tens of KB | Yes | Per-component config | Rejected. Capability not needed. |
| React Spring | Tens of KB | Yes | Per-component config | Rejected. Same, plus physics this UI avoids. |
| View Transitions API | 0 | Partly | Media query | Deferred. Nothing to animate yet. |
| CSS with hand-written keyframes only | 0 | No | One media query | Rejected. `tw-animate-css` is the same thing, maintained. |

### Motion, formerly Framer Motion

The strongest alternative and the default choice in most React codebases. It does layout animation, shared-element transitions, gestures, spring physics, and exit animations with a clean API, and `AnimatePresence` solves the unmount problem elegantly.

Rejected because every capability it adds is one this interface has decided not to use. There are no layout animations, no gestures, no springs, and no list-item enter or exit — the specification rules out the last one on virtualization grounds. What remains is fades, small translations, and a width transition, all of which CSS does in fewer bytes and on the compositor. `AnimatePresence`'s main advantage, animating an element out before unmount, is now available in CSS through `@starting-style` and `allow-discrete`.

The cost side is real too: a JavaScript animation loop competes for the main thread with SSE handling and virtual list work, which is the thread that determines whether this UI feels live.

### React Spring

Rejected for the same reasons as Motion, with an additional mismatch: its spring model is well suited to playful, physical interfaces, and this specification explicitly wants motion that explains rather than delights. Fixed durations with defined easing are the intent.

### The View Transitions API

Same-document view transitions are Baseline and would be a zero-dependency way to animate route changes.

Deferred rather than rejected, because the specification says route changes have **no** transition — content replaces with a skeleton after 150 ms. There is nothing to animate. Recorded here so the platform fact is on file, with one caveat for any future plan: **cross-document** view transitions are not Baseline, since Firefox and Safari have not implemented them, so a design depending on them would be built on support that does not exist.

### Hand-written keyframes without a utility package

Rejected only as duplicated effort. `tw-animate-css` provides the same CSS as maintained utilities that compose with Tailwind v4, and nothing about it prevents writing bespoke keyframes where the utilities do not fit.

## Revisit this decision when

- **A design calls for shared-element or layout animation.** That is the one thing CSS cannot do well, and it would justify a library for that interaction alone rather than globally.
- **Gesture-driven interaction is added**, such as swipe-to-dismiss on touch.
- **Progress interpolation looks wrong at the real SSE rate** and cannot be fixed by tuning duration or easing. A small interpolation helper is a better answer than an animation library.
- **Route transitions become desirable.** Same-document View Transitions are the zero-dependency answer; check current support before assuming the cross-document variant.
- **`@starting-style` support becomes a reported problem** for a browser in actual use.

## References

- [`tw-animate-css`](https://www.npmjs.com/package/tw-animate-css) — the replacement for `tailwindcss-animate`
- [shadcn/ui: Tailwind v4](https://ui.shadcn.com/docs/tailwind-v4) — records the deprecation
- [MDN: `@starting-style`](https://developer.mozilla.org/docs/Web/CSS/@starting-style)
- [MDN: `transition-behavior`](https://developer.mozilla.org/docs/Web/CSS/transition-behavior)
- [MDN: View Transitions API](https://developer.mozilla.org/docs/Web/API/View_Transitions_API)
- [The View Transitions API in 2026](https://brainstormsandraves.com/css/view-transitions-2026/) — directional; Baseline status of same- versus cross-document
- [MDN: `prefers-reduced-motion`](https://developer.mozilla.org/docs/Web/CSS/@media/prefers-reduced-motion)
- [WCAG 2.2: 2.3.3 Animation from Interactions](https://www.w3.org/WAI/WCAG22/Understanding/animation-from-interactions.html)
