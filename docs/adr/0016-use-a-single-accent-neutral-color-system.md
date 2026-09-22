# ADR-0016: Use a single-accent neutral color system

| | |
|---|---|
| **Status** | Accepted, with required token corrections |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/app/theme.css` |
| **Supersedes** | None |

This article explains why the interface uses one accent hue over an authored neutral scale, with semantic colors restricted to dots, text, and thin bars. It also records the result of checking the proposed token table against the WCAG 2.2 AA targets the design specification commits to: **four tokens do not meet them as specified**, and one of those is the focus ring.

## Context

### The interface is dense, and color has to stay quiet

The design principles put information first and state that every pixel of chrome must justify itself against a table row. A dense table with 32-pixel rows, a status column, progress bars, selection, hover, and focus states has a limited budget for color before it stops reading as data and starts reading as decoration.

There is a second pressure in the opposite direction: job state has five values (`queued`, `running`, `succeeded`, `failed`, `cancelled`), and the SSE stream changes them in place. A conventional approach would give each state its own hue, which is how a table acquires six competing colors.

### Both themes are authored, not derived

The specification states this as a principle: tokens are authored per theme, not inverted. A derived dark theme produces muddy grays and inverts contrast relationships that were tuned for light. So there are two independently authored scales, which means twice the surface to verify.

### WCAG 2.2 AA is a commitment with numbers

The accessibility section commits to contrast of at least **4.5:1 for text** and **3:1 for UI**, checked in CI with `axe-core`. It also commits to visible focus everywhere, with a 2px `--ring` outline at offset 2, and states that color is never the only signal.

That commitment is testable, so this ADR tested it.

### OKLCH lightness is not WCAG luminance

The token table is authored in OKLCH and reasons about lightness percentages — semantic colors are specified as "teal / amber / red at 55% L" in light and "the same hues at 70% L" in dark. That reasoning is perceptually sound but it does **not** predict WCAG contrast, because WCAG 2.x uses relative luminance, which varies with hue at constant OKLCH lightness.

The measurements below show it directly: `success`, `warning`, and `danger` all sit at 55% L on white and produce **4.46:1, 4.95:1, and 5.38:1**. Same lightness, three different ratios, straddling the threshold.

## Decision

Adopt the single-accent neutral system as specified, with the corrections below applied before `theme.css` is written.

- **One accent hue** at 262 (indigo-blue) for selection, primary buttons, active navigation, and progress.
- **A neutral scale with a slight cool cast** so grays do not look muddy in dark mode.
- **Semantic colors appear only as 6px dots, as text, or as 2px bars — never as filled surfaces.** This is the rule that makes five job states legible under one accent, and it is the load-bearing constraint of the whole system.
- **Elevation is border plus background step**, not shadow, except one `shadow-sm` on popovers and dialogs.
- **Authored per theme** in OKLCH, exposed as Tailwind v4 `@theme` variables with shadcn-compatible names.
- **Color is never the only signal**: every status dot is paired with text or an `aria-label`.

### Required corrections

These were found by computing WCAG contrast from the proposed OKLCH values. Ratios assume sRGB clamping; see the gamut caveat below.

| Token | As specified | Measured | Required | Action |
|---|---|---|---|---|
| `--ring` | `--accent` at 60% alpha | **1.92:1** on light surface | 3:1 | **Make the ring opaque.** Opaque accent gives 5.00:1. |
| `--success` | `oklch(55% 0.13 165)` | **4.46:1** on white | 4.5:1 | Lower to `oklch(53% …)` → 4.82:1. |
| `--border-strong` | `oklch(84% 0 0)` | **1.63:1** on white | 3:1 *for its stated use* | See below. |
| `--border` | `oklch(92% 0 0)` | 1.27:1 on white | Exempt if decorative | Keep, with a constraint. |

> [!WARNING]
> The focus ring is the most serious of these. `--accent` at 60% alpha over a light surface computes to **1.92:1**, well under the 3:1 that WCAG 1.4.11 requires of a focus indicator, and focus visibility is also a hard requirement under 2.4.7. The same ring passes in dark mode (4.63:1), which is exactly how this kind of defect ships — it looks fine in the theme the developer works in. Use an opaque `--accent` for `--ring` in both themes.

**On `--border-strong`:** the specification assigns it to "focused inputs, dividers that must read". A border that communicates the focused state of an input *is* a UI component state indicator and needs 3:1, which at `oklch(84% 0 0)` it does not have. Two acceptable resolutions:

1. **Keep the border decorative and let the ring carry focus.** With an opaque accent ring at 5.00:1, the focus state is conveyed by the ring, and `--border-strong` needs no contrast guarantee. Recommended, because it is one mechanism rather than two.
2. **Darken it to roughly `oklch(66% 0 0)`** (3.11:1) if the border itself must indicate state.

**On `--border`:** 1.27:1 is fine. WCAG 1.4.11 exempts purely decorative visual information, and a hairline between table rows is decorative as long as the rows are identifiable without it. The constraint to hold is that `--border` must never be the only thing distinguishing a component or a state — which the design principles already require.

### The gamut caveat

Five tokens fall outside the sRGB gamut as written: in light, `--accent-soft`, `--success`, and `--warning`; in dark, `--accent` and `--danger`.

Browsers gamut-map out-of-range OKLCH colors by reducing chroma, so the **rendered** color is not the specified one, and its contrast is not the computed one. The measurements above approximate this by clamping, which is close but not identical to CSS Color 4 gamut mapping.

> [!IMPORTANT]
> Because of this, contrast must be verified against **rendered** output, not computed from token values. That is what makes the `axe-core` checks the design specification already plans the authoritative test, and this ADR's numbers a first pass that found four problems rather than a certificate that the rest are fine.

## Consequences

### What you gain

- **Five job states stay readable under one hue.** Restricting semantics to dots, text, and thin bars means state is conveyed by a small, high-contrast mark next to text, not by six competing surface colors.
- **Data reads as data.** With one accent, a colored cell means something specific, so the table's information density is not fighting its own palette.
- **Dark mode is designed.** Authoring both scales avoids the inverted-relationship artifacts of a derived theme, and the measurements confirm dark mode is comfortably above threshold nearly everywhere.
- **Fewer decisions later.** A new status or badge inherits an existing rule rather than needing a new hue.
- **`shadow`-free elevation composes with borders**, which matters in forced-colors mode where shadows disappear but borders survive.
- **The token table is testable**, and now partially tested.

### What it costs you

- **OKLCH lightness gives no contrast guarantee.** Every semantic color needs measuring individually, and "the same hues at 70% L" is a design statement, not a compliance statement.
- **Five tokens are gamut-mapped**, so the specification and the render differ, and only the render counts.
- **Two scales to verify, and one of them is the one nobody develops in.** The focus-ring defect exists in light mode only; without a test in both themes it would have shipped.
- **A single accent restricts expressiveness.** There is no second hue available for a future feature that wants its own identity, and adding one would reopen this decision.
- **The dot-not-fill rule is a convention the type system cannot enforce.** A filled danger button would look normal in a pull request and would break the system's central premise.

### Follow-up work this decision creates

1. **Apply the four corrections** before writing `theme.css`: opaque ring, `--success` at 53% L, the `--border-strong` resolution, and the documented decorative status of `--border`.
2. **Add a contrast test over rendered colors**, asserting every foreground-on-background pair in the token table meets 4.5:1 and every UI pair meets 3:1, **in both themes**. Run it on computed styles so gamut mapping is included.
3. **Test the focus ring specifically**, in both themes, on `--surface` and `--surface-raised`. This is the pair that failed.
4. **Add a lint or review rule against semantic-colored fills**, so the dot-not-fill rule survives contributors.
5. **Verify `--accent-soft` as a selected-row background** with `--foreground` on top. It measures 15.60:1 light and 13.08:1 dark, which is fine, but it is gamut-mapped in light and should be confirmed on render.
6. **Check forced-colors mode** keeps borders and focus visible, as the specification requires.

## Alternatives considered

| Alternative | Hues in the table | Outcome |
|---|---|---|
| Single accent, semantics as dots and bars | 1 + 3 marks | **Chosen** |
| Multi-hue semantic palette with filled badges | 4–6 | Rejected. Destroys density legibility. |
| shadcn default palette unmodified | 1 + defaults | Rejected. Not authored per theme; not measured. |
| HSL or hex authoring | — | Rejected. Worse perceptual uniformity, same contrast problem. |
| APCA instead of WCAG 2.x contrast | — | Rejected. Not the standard being committed to. |

### A multi-hue semantic palette with filled badges

The conventional approach: a colored pill per job state, each with its own hue and a filled background. It is immediately readable in isolation and requires no discipline to maintain.

Rejected because it fails at density. Thirty rows with filled state badges puts thirty saturated rectangles in a table whose purpose is to let the eye scan numbers and titles, and it makes the accent — which marks selection and progress — compete with decoration. Filled badges also multiply the contrast surface: every fill needs its foreground checked against it, in both themes, which is four times the verification of a dot next to ordinary text.

### The shadcn default palette, unmodified

Rejected on two grounds. It derives dark from light rather than authoring both, which the design principles explicitly reject; and its values would still need the same measurement work, since inheriting a palette does not inherit a contrast guarantee for this application's specific pairings.

### HSL or hex authoring

Rejected because OKLCH's perceptual uniformity is genuinely useful for building a neutral ramp and for keeping the three semantic hues visually balanced. Note that this choice does not help with contrast — as measured above, equal OKLCH lightness produced ratios from 4.46 to 5.38 — but it makes the *design* work coherent, and the contrast work has to be measured under any color space.

### APCA instead of WCAG 2.x

APCA models perceptual contrast better than the WCAG 2.x luminance ratio, particularly for light text on dark backgrounds, and it is the candidate method for WCAG 3.

Rejected because the specification commits to WCAG 2.2 AA, that is what `axe-core` measures, and it is the standard an accessibility audit would apply. APCA is worth consulting as a design aid when a pair passes WCAG but looks wrong; it is not the acceptance criterion.

## Revisit this decision when

- **A rendered-color contrast test disagrees with the numbers here.** The rendered values are authoritative; these were computed with clamping.
- **A feature needs a second identity color.** Adding a hue means re-verifying the semantic marks against it and reopening the density argument.
- **Forced-colors or high-contrast mode reveals a state that depends on color alone.** That is a violation of the existing principle, not a palette problem, but it surfaces here.
- **WCAG 3 and APCA become the standard being audited against.** Then the thresholds change and the whole table is re-measured.
- **Out-of-gamut tokens render visibly differently across browsers.** Pulling chroma inside sRGB would trade a little vividness for predictability, which for a data-dense interface is likely the right trade.

## References

- [WCAG 2.2](https://www.w3.org/TR/WCAG22/) — 1.4.3 contrast (minimum), 1.4.11 non-text contrast, 2.4.7 focus visible
- [Understanding non-text contrast, including the decorative exemption](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)
- [CSS Color 4: `oklch()`](https://www.w3.org/TR/css-color-4/#ok-lab)
- [CSS Color 4: gamut mapping](https://www.w3.org/TR/css-color-4/#gamut-mapping)
- [Tailwind CSS v4: `@theme`](https://tailwindcss.com/docs/theme)
- [`axe-core`](https://www.npmjs.com/package/axe-core)
- [APCA](https://git.apcacontrast.com/) — consulted, not adopted
