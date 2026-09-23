# Design mockups

Visual references produced before the frontend was built. They are **not**
specifications: where a mockup and an ADR disagree, the ADR is the decision
and the mockup is a picture of one way it could look.

## `ui-mockup.html`

A single self-contained page that unpacks four screens, each itself a
standalone HTML document with its assets inlined. Open it in a browser; there
is no build step and no network access.

| Screen | Relates to |
|---|---|
| Library — master detail | [ADR-0017](../adr/0017-address-the-detail-panel-with-a-nested-route.md), which decides the panel is a nested route rather than a search parameter |
| Downloads — queue with live progress | [ADR-0010](../adr/0010-use-server-sent-events-for-live-updates.md) — the progress is what SSE carries |
| Browse — source catalog grid | The catalog routes, and [ADR-0004](../adr/0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md) for what a source can offer |
| Command palette | Not yet covered by any ADR |

### Three things to check a mockup against before building from it

- **Colour.** [ADR-0016](../adr/0016-use-a-single-accent-neutral-color-system.md)
  records four token corrections found by *measuring* the proposed palette
  against WCAG 2.2 AA — including a focus ring at 1.92:1. A mockup is not
  evidence that a colour passes; the contrast test over rendered tokens is.
- **Motion.** [ADR-0018](../adr/0018-keep-motion-in-css.md) keeps animation in
  CSS with no library. A mockup that implies otherwise is a picture, not a
  dependency decision.
- **Capability.** A control in a mockup is not a commitment to the capability
  behind it. These screens are the authority on layout, density, spacing and
  colour — on how the interface is distributed — and say nothing about what
  the server can do.

  Three elements have been built past on that basis, each recorded where it
  was skipped rather than silently dropped:

  | Element | Why it is not there |
  |---|---|
  | "Unread 32" and "14 new" badges | Nothing records what has been read. The screens show what is *downloaded* instead, which the server can defend. |
  | "Refresh metadata" in the command palette | `POST /jobs` accepts four maintenance kinds and answers 400 for that one. |
  | "Download 14 new chapters" | One request per chapter behind a single keypress; the detail panel does it per chapter, where the reader sees what they are asking for. |

  When an element implies a capability the API does not have, adding the
  capability is its own decision. Until it is made, the element is not built
  — and not faked from a number that happens to be nearby.

### Why it is committed as one 1.2 MB file

It is a generated artifact with gzipped bundles inside, so it does not diff
usefully and splitting it would mean regenerating something nobody has the
generator for. It is committed once as a reference and is not expected to
change; if the design moves, replace it rather than editing it.
