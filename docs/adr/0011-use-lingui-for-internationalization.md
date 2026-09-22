# ADR-0011: Use Lingui for internationalization

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/shared/i18n`, `web/vite.config.ts` |
| **Supersedes** | None |

This article explains why translation uses Lingui with compile-time message extraction rather than react-i18next, and records the one build decision that comes with it: macros are transformed by Babel, not by the experimental SWC plugin.

## Context

The application ships English as the source locale and Spanish alongside it from the first release, so that extraction, plural handling, and layout tolerance are exercised rather than assumed.

### The design system already states the i18n requirements

The design specification is explicit, and these are constraints rather than preferences:

- **Strings are never concatenated.** Every user-visible sentence is one message.
- **Plurals go through ICU MessageFormat**, so a count and its noun form stay in one message that a translator can adapt to their language's plural rules.
- **A `lang` attribute is set per locale.**
- **Layout tolerates roughly 30% longer strings**, which is a UI constraint but depends on translations being complete enough to test with.
- **Right-to-left is declared out of scope** for v1.

### Missing translations should fail the build, not the UI

The frontend CI pipeline runs `lingui extract --clean` as a check between linting and tests. The intent is that an untranslated string is a build failure, not a fallback rendered to a user. That is only possible with a tool that extracts messages from source at build time.

### The build is Vite 8

Per the correction recorded in [ADR-0008](0008-use-tanstack-router-and-query-for-routing-and-data.md), the toolchain is Vite 8, which means Rolldown. Anything that needs to transform source before bundling has to fit that pipeline.

## Decision

Use **Lingui 6.7.x** with `@lingui/vite-plugin`.

- **Source text is the message ID.** Write `t\`Download queued\`` and `<Trans>` in components; do not maintain a separate key namespace.
- **Plurals use the `plural` macro**, producing ICU MessageFormat in a single message.
- **Catalogs are `.po` files** under `shared/i18n/catalogs/{en,es}/messages.po`, compiled at build time and lazy-loaded per locale.
- **Locale resolution order:** the user's stored preference, then `navigator.language`, then the source locale.
- **Dates and numbers go through `Intl`**, not through message catalogs. A formatted date is not a translatable string.
- **`lingui extract --clean` runs in CI** and fails on drift, which is what makes an untranslated string a build error.

### Transform macros with Babel, not the SWC plugin

Lingui macros require a compile-time transform. There are two paths, and they are not equivalent in risk:

| Path | Status |
|---|---|
| Babel, via `linguiTransformerBabelPreset` shipped in v6 | Supported. The preset exists specifically to wire Babel into a Rolldown-based Vite build. |
| `@lingui/swc-plugin` | **Officially experimental.** It does not guarantee semver compatibility across `swc_core` versions, and version mismatches with `@swc/core` are a known, reported failure mode. |

> [!IMPORTANT]
> **Verified wiring (Vite 8 + plugin-react 6):** `@vitejs/plugin-react` 6.x transforms JSX with Oxc and has **no `babel` option at all** — Babel is no longer part of that plugin. The Babel pass arrives as a separate Rolldown plugin:
>
> ```ts
> import babel from '@rolldown/plugin-babel'
> import { lingui, linguiTransformerBabelPreset } from '@lingui/vite-plugin'
> plugins: [react(), lingui(), babel({ presets: [linguiTransformerBabelPreset()] })]
> ```
>
> `@rolldown/plugin-babel` is an optional peer of `@lingui/vite-plugin` and must be installed explicitly. Two more things confirmed by building: Lingui 6 no longer accepts `format: 'po'` as a string — formatters are separate packages (`@lingui/format-po`, used as `format: formatter({ lineNumbers: false })`) — and `lingui()` takes `failOnMissing` and `failOnCompileError`, which enforce translation completeness at build time more directly than the CI `extract --clean` check alone.
>
> Use the Babel preset. An SWC plugin whose compatibility depends on matching internal `swc_core` versions is a build that breaks on an unrelated dependency bump, and the failure surfaces as a confusing transform error rather than a version conflict. Revisit only when the SWC plugin declares stability.

This is the real cost of choosing Lingui: Babel re-enters a build that would otherwise not need it.

## Consequences

### What you gain

- **No key management, so keys cannot drift from copy.** With source text as the ID, there is no `downloads.queued.title` to keep in step with the sentence it names, and a translator sees the actual English rather than a dotted path.
- **Completeness is checkable at build time.** `extract --clean` compares source against catalogs, so an added string with no translation is caught in CI. A runtime lookup library can only fall back.
- **ICU plurals in one message.** The `plural` macro keeps the count and every plural form together, which is what the design specification requires and what makes Spanish and later locales correct rather than approximated.
- **A small runtime.** Compiled catalogs plus a minimal runtime, with per-locale lazy loading, so adding locales does not grow the initial bundle.
- **`.po` is standard translator tooling.** Poedit, Weblate, and commercial platforms consume it directly, with no custom export step.

### What it costs you

- **Babel is in the build.** On a Vite 8 and Rolldown pipeline this is the one slow transform, and it exists solely for i18n macros. Mitigated by the official preset, but it is real.
- **Editing English copy invalidates a translation.** Because the source string is the ID, fixing a typo orphans the Spanish message. The `.po` workflow handles this through fuzzy matching, but it is a workflow contributors must understand rather than an automatic behavior.
- **Extraction is a step people forget.** A new string without a run of `extract` shows as a CI failure — which is the intended design, but it is friction on every branch that adds copy.
- **A smaller ecosystem than i18next.** Fewer framework integrations, fewer answers, fewer plugins. For a two-locale application this is affordable.
- **No help with right-to-left.** Out of scope by declaration, and no i18n library would change that; the cost is layout work if it ever comes into scope.

### Follow-up work this decision creates

1. **Wire `linguiTransformerBabelPreset`** in `vite.config.ts` and confirm the production build and the dev server both transform macros.
2. **Add the `extract --clean` CI gate** in the position the pipeline already specifies, and make its failure message point at the fix.
3. **Set the `lang` attribute** from the active locale on the `html` element, alongside the theme attribute set before first paint.
4. **Write the pseudo-localization or 30%-expansion check** the design specification implies, so layout tolerance is tested rather than asserted. A generated long-string locale is the cheapest way to do this.
5. **Document the copy-edit workflow**: change English, re-extract, mark the Spanish message fuzzy, retranslate.
6. **Add a lint rule or review rule against string concatenation** in user-visible text, since this is a design-system requirement that neither Lingui nor the type system enforces.

## Alternatives considered

| Alternative | Latest (Sept 2026) | Extraction | Build transform | Outcome |
|---|---|---|---|---|
| Lingui | 6.7.0 | Compile-time, source as ID | Babel or experimental SWC | **Chosen** |
| react-i18next | 17.0.15 / i18next 26.4.2 | None; manual keys | None | Rejected. No build-time completeness guarantee. |
| react-intl (FormatJS) | 12.1.2 | Compile-time, explicit IDs | Babel or SWC | Rejected. More verbose for the same benefit. |
| Paraglide JS | 2.25.4 | Compile-time, tree-shaken | Its own compiler | Rejected for now; closest thing to revisit. |

### react-i18next

This is the most-used and most actively maintained option — `i18next` 26.4.2 and `react-i18next` 17.0.15 were both published within weeks of this decision — and it has the largest ecosystem by a wide margin. It also needs **no build transform at all**, which means no Babel in a Rolldown pipeline. If build simplicity were the deciding criterion, it would win.

It was rejected on two grounds:

1. **Keys are managed by hand.** A namespaced key and the sentence it refers to are separate artifacts that drift, and a translator receives the key rather than the source text. In a UI this dense — tables, toasts, empty states, command palette entries — that drift accumulates quietly.
2. **There is no build-time completeness check.** Missing translations fall back at runtime, which is exactly the behavior the CI gate is meant to prevent. Third-party extraction tooling exists, but it is bolted on rather than the library's model.

ICU plural support is also secondary rather than native, requiring a separate format plugin, whereas the design specification names ICU directly.

### react-intl (FormatJS)

FormatJS is the reference ICU implementation for JavaScript and is mature and well maintained. It extracts at compile time like Lingui and supports the same plural semantics.

Rejected on ergonomics rather than capability: message descriptors with explicit IDs and `defaultMessage` are more verbose at every call site than Lingui's macros, and it reintroduces the manual ID surface that Lingui removes, for the same build-transform cost.

### Paraglide JS

Paraglide compiles each message into a tree-shakeable function, giving per-message type safety and no runtime catalog lookup. It is the most interesting of the alternatives and the one to watch.

Rejected because its message format and plural handling differ from ICU, which the design specification names explicitly, and because the translator tooling around it is younger than the `.po` ecosystem. If ICU stops being a hard requirement, this becomes a serious candidate.

## Revisit this decision when

- **`@lingui/swc-plugin` declares stability.** Dropping Babel from the build is the single biggest improvement available to this decision.
- **Locale count grows past a handful**, or professional translators join the workflow. Both make the `.po` and extraction model more valuable, reinforcing rather than reversing the choice.
- **Right-to-left comes into scope.** That is layout and design work, but it is the moment to re-examine whether the i18n layer helps or is merely neutral.
- **The Babel transform becomes a measurable build bottleneck** and the SWC plugin is still experimental. Then react-i18next's zero-transform model is worth reconsidering against the completeness guarantee.

## References

- [Announcing Lingui 6.0](https://lingui.dev/blog/2026/04/22/announcing-lingui-6.0) — Vite 8 support and `linguiTransformerBabelPreset`
- [Lingui installation and setup](https://lingui.dev/installation)
- [Lingui: comparison with i18next](https://lingui.dev/misc/i18next)
- [`@lingui/swc-plugin`](https://www.npmjs.com/package/@lingui/swc-plugin) — experimental status and `swc_core` compatibility
- [`i18next`](https://www.npmjs.com/package/i18next) and [`react-i18next`](https://www.npmjs.com/package/react-i18next)
- [`react-intl`](https://www.npmjs.com/package/react-intl)
- [Paraglide JS](https://www.npmjs.com/package/@inlang/paraglide-js)
- [ICU MessageFormat](https://unicode-org.github.io/icu/userguide/format_parse/messages/)
