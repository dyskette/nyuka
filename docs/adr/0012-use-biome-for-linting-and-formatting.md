# ADR-0012: Use Biome for linting and formatting

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/` |
| **Supersedes** | None |

This article explains why the frontend uses Biome as a single linter and formatter instead of ESLint with Prettier, how the project's two custom conventions are expressed, and why static linting does not discharge the accessibility requirements in the design specification.

## Context

### Two ADRs have already asked for custom lint rules

This matters more than general rule coverage, because it is a capability question rather than a preference:

- **[ADR-0009](0009-hand-write-query-hooks-with-semantic-key-factories.md)** requires a rule banning inline query-key array literals outside `features/*/api/`. Without it, a key that differs by one segment creates a second cache entry that live updates never reach, and the symptom appears months later as a view that silently stopped refreshing.
- **[ADR-0008](0008-use-tanstack-router-and-query-for-routing-and-data.md)** wants the loader convention enforced: loaders call `ensureQueryData` and return nothing components read.

A linter that cannot express project-specific conventions fails the first real requirement placed on it here.

### The design specification sets accessibility targets a linter cannot verify

The specification commits to WCAG 2.2 AA and names specific criteria: contrast ratios of at least 4.5:1 for text and 3:1 for UI, focus not obscured by the sticky header or status bar (2.4.11), target size at least 24×24 in compact density (2.5.8), keyboard equivalents for drag interactions (2.5.7), `role="grid"` with roving tabindex and `aria-rowcount` on virtual lists, and throttled live regions.

Almost none of that is statically checkable. Contrast depends on computed styles, focus obstruction depends on layout, and target size depends on rendering. The specification already plans `axe-core` in both Vitest and Playwright, which is the right instrument. A linter's role is the subset that *is* structural.

### The pipeline expects one fast step

The frontend CI job runs install → lint → `tsc --noEmit` → Lingui check → Vitest → build → Playwright. Linting is the first gate, so it should be fast and it should not have opinions that conflict with the formatter.

## Decision

Use **Biome 2.5.x** as the only linter and formatter for `web/`. Do not install ESLint or Prettier.

- **Enable the `a11y` rule group at error severity**, covering the structural accessibility rules — `useKeyWithClickEvents`, `useButtonType`, and the rest of the group.
- **Enable the `react` domain.** Biome's domains auto-enable technology-specific rules when it detects the dependency in the nearest `package.json`, which brings in the React hooks rules.
- **Rely on `useExhaustiveDependencies` and `useHookAtTopLevel`** as the hooks rules. These are Biome's equivalents of the `eslint-plugin-react-hooks` rules; some secondary sources claim no equivalent exists, which is out of date.
- **Express the two project conventions as GritQL plugins.** Biome's plugin system queries the CST and reports custom diagnostics, which is sufficient for both: matching array literals in query-key positions, and matching loader return statements.
- **Keep `axe-core` in Vitest and Playwright.** The lint `a11y` group and runtime axe checks are complementary, not redundant. Neither alone satisfies the design specification.
- **Use Biome's formatter as the single source of formatting truth**, with no second formatter and no format-related lint rules.

> [!IMPORTANT]
> The GritQL plugin for the query-key rule from ADR-0009 is not optional polish. It is the mechanism that makes that ADR's central invariant enforceable. If it turns out not to be expressible in GritQL, that is grounds to revisit this decision rather than to drop the rule.

## Consequences

### What you gain

- **One tool, one configuration, one pass.** Linting and formatting cannot disagree, because they are the same binary reading the same config. No `eslint-config-prettier` shim and no arguments about which tool owns a rule.
- **No plugin version matrix.** The recurring maintenance cost of an ESLint setup is keeping the core, the TypeScript parser, and each plugin on mutually compatible versions. That cost is absent here, and Biome upgrades are correspondingly low-drama because there is no third-party plugin surface to break.
- **Type-aware linting without invoking `tsc`.** Biome v2 introduced type-informed rules independent of the TypeScript compiler, so rules that previously needed a type-aware ESLint setup do not add a second type-check pass to the pipeline.
- **Speed at the first gate.** A Rust implementation on a single pass, which keeps the fastest-failing check actually fast.
- **Custom conventions are expressible.** GritQL plugins cover the two rules this project needs, which was the binding requirement.
- **Domains reduce configuration.** React rules turn on because React is a dependency, rather than because someone remembered to extend a config.

### What it costs you

- **Roughly 80% of common ESLint rules, and no long tail.** Biome covers the widely used rules and adds more each release. A niche rule, or a rule from a framework plugin without a Biome counterpart, has no equivalent — and the remedy is waiting for the core team rather than writing or finding a plugin.
- **GritQL is pattern matching, not arbitrary code.** It queries the CST with captures, operators, and `register_diagnostic`, which is enough for structural conventions. It is not enough for a rule needing real control flow or type resolution of its own. ESLint's JavaScript rule API has no such ceiling.
- **Markdown has no resources allocated** on Biome's 2026 roadmap, and HTML formatting is still working toward Prettier parity. If repository Markdown or HTML ever needs formatting, that is a separate tool.
- **Accessibility linting is a small fraction of the accessibility requirement.** The `a11y` group is real value, but treating a green lint run as WCAG conformance would be a serious misreading. The runtime axe checks are the load-bearing control.
- **A smaller answer pool.** Fewer Stack Overflow answers and fewer blog posts than ESLint, and some of what exists is out of date in both directions.

### Follow-up work this decision creates

1. ~~**Write the ADR-0009 query-key GritQL plugin first.**~~ **Done and verified.** GritQL can express it, so this decision holds. The working pattern is in `web/biome-plugins/no-inline-query-keys.grit`:

   ```grit
   `queryKey: $key` where {
     $key <: `[$...]`,
     register_diagnostic(span = $key, message = "…", severity = "error")
   }
   ```

   Two things that did not work and cost time: an object pattern (`{ queryKey: [$_] }`) does not match when the object has other properties, and a node-kind constraint (`array_expression()`) fails to load. Match the property and constrain the value. Confirmed to fire on `queryKey: ['jobs']`, stay silent on `queryKey: jobsKeys.detail(id)`, and need no path exemption — key factories declare arrays under their own property names, not under `queryKey:`.
2. **Write the ADR-0008 loader-convention plugin**, or record explicitly that it is review-enforced if the pattern proves awkward to match.
3. **Pin Biome exactly** and treat upgrades as reviewed changes, since new rules arriving at error severity can fail CI on an unrelated branch.
4. **Wire `axe-core` into both Vitest and Playwright** as the design specification requires, and make clear in `web/README.md` that lint `a11y` and axe cover different things.
5. **Add the contrast check** against the token table in the design specification, so a token edit that breaks 4.5:1 fails a test rather than shipping.
6. **Document the one-formatter rule** so nobody adds Prettier for "just this file type".

## Alternatives considered

| Alternative | Latest (Sept 2026) | Lint + format | Custom rules | Outcome |
|---|---|---|---|---|
| Biome | 2.5.14 | Both, one tool | GritQL plugins | **Chosen** |
| ESLint + Prettier | 10.11.0 / 3.9.8 | Two tools | Arbitrary JS | Rejected. More capable, more to maintain. |
| oxlint (+ a formatter) | 1.85.0 | Lint only | Limited | Rejected. Still needs a second tool. |
| Biome format + oxlint lint | — | Two tools | Limited | Rejected. Two tools with overlapping scope. |

### ESLint + Prettier

This is the more capable option and should be described as such. ESLint 10.11.0 has the largest rule ecosystem in the JavaScript world, framework plugins that Biome has no counterpart for, and — decisively for the criterion this ADR opened with — an **unrestricted custom rule API**. A convention that GritQL cannot match is a straightforward ESLint plugin written in JavaScript with full access to the AST and scope analysis.

It was rejected on total maintenance cost rather than capability:

1. **Two tools with an overlapping boundary.** Formatting-adjacent lint rules must be disabled so the formatter can own them, which is a configuration layer that exists only because the tools are separate.
2. **The plugin version matrix is the recurring tax.** ESLint core, the TypeScript parser and plugin, the React plugins, and the a11y plugin all have to agree, and a major bump in any of them is a coordinated upgrade. With TypeScript on 7.x and React on 19.3, that surface is actively moving.
3. **Speed at the first gate.** ESLint with type-aware rules is the slowest plausible configuration of the first CI step.

Because GritQL covers the two conventions this project actually needs, the extra capability is not currently paying for the extra cost. That calculation is why the follow-up list puts writing the plugin first.

### oxlint

oxlint 1.85.0 is the fastest of the three and comes from the same ecosystem as Vite and Rolldown, which is a real alignment argument given the build in ADR-0008.

Rejected because it is a linter only. Adopting it means pairing it with Prettier, dprint, or Biome's formatter — back to two tools, which is the specific cost this decision is avoiding. Its rule coverage and custom-rule story are also still maturing relative to both alternatives.

### Biome for formatting, oxlint for linting

Rejected as the worst of both: two tools with overlapping scope, two upgrade cadences, and a formatting/linting boundary to police, in exchange for speed that is not the bottleneck.

## Revisit this decision when

- **A needed convention cannot be expressed in GritQL.** This is the most likely trigger. The response is not necessarily a full switch: ESLint can be added for one plugin, at the cost of the single-tool property.
- **React's compiler-era hooks rules become essential** and Biome's equivalents lag materially behind `eslint-plugin-react-hooks`.
- **New rules at error severity repeatedly break CI on unrelated branches**, indicating the pin-and-review discipline is not holding.
- **Repository Markdown or HTML needs formatting.** Markdown is explicitly unresourced on the roadmap, so that would mean a second tool regardless.
- **oxlint ships a formatter** with comparable coverage, which would make it a direct single-tool competitor.

## References

- [Biome linter](https://biomejs.dev/linter/)
- [Biome: linter plugins (GritQL)](https://biomejs.dev/linter/plugins/)
- [Biome: `useExhaustiveDependencies`](https://biomejs.dev/linter/rules/use-exhaustive-dependencies/)
- [Biome: `useHookAtTopLevel`](https://biomejs.dev/linter/rules/use-hook-at-top-level/)
- [Biome roadmap 2026](https://biomejs.dev/blog/roadmap-2026/) — plugin system, cross-file analysis, Markdown unresourced
- [`@biomejs/biome`](https://www.npmjs.com/package/@biomejs/biome) — 2.5.14
- [ESLint](https://www.npmjs.com/package/eslint) — 10.11.0
- [oxlint](https://www.npmjs.com/package/oxlint) — 1.85.0
- [WCAG 2.2](https://www.w3.org/TR/WCAG22/) — criteria 2.4.11, 2.5.7, 2.5.8
- [`axe-core`](https://www.npmjs.com/package/axe-core)
