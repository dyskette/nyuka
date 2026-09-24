# ADR-0004: Use Aidoku WASM sources as the provider mechanism

| | |
|---|---|
| **Status** | Accepted, with scoped capability gaps |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/aidoku-runtime`, `crates/domain` (provider ports) |
| **Supersedes** | None |

This article explains why manga providers are [Aidoku](https://aidoku.app) `.aix` extensions executed in a `wasmtime` sandbox, what the Aidoku host ABI actually requires you to implement, and which parts of it this project will not implement in v1. It also corrects three assumptions carried in the architecture draft, because reading the ABI changed them.

> [!IMPORTANT]
> The Aidoku source API is not a versioned, published contract. The `aidoku` crate is `publish = false`, has no git tags, and is consumed by sources as a git dependency. There is no semver boundary to pin against, so this ADR defines the project's own ABI versioning discipline instead. Read [Pin the ABI by commit](#pin-the-abi-by-commit-not-by-version) before writing any host import.

## Context

The application reads manga from third-party sites. Those sites change markup without notice, add bot protection, and are numerous enough that hand-maintaining a scraper per site is open-ended work. So the provider layer has to satisfy four requirements:

- **Providers are installed and updated at runtime**, from a repository index, without rebuilding or redeploying the API.
- **Provider code is untrusted.** It comes from a third-party repository, it is not reviewed by this project, and it must not be able to read the filesystem, open arbitrary sockets, reach the Postgres instance, or contact cloud metadata endpoints.
- **An existing ecosystem supplies the providers.** Writing site adapters is the work this decision is meant to avoid.
- **Bot protection is handled somewhere.** Several target sites sit behind Cloudflare.

`crates/domain` declares the provider ports — `SourceCatalog`, `SourceItem`, and `SourceRegistry` — so any mechanism chosen here is an adapter behind those traits, and native Rust adapters remain possible for individual sites.

### What the Aidoku ABI actually requires

The architecture draft listed the host modules to implement as "net, html, defaults, storage, std, js, canvas". Reading `aidoku-rs` confirms the module list exactly — `crates/lib/src/imports/` contains `net.rs`, `html.rs`, `defaults.rs`, `std.rs`, `js.rs`, `canvas.rs`, and `error.rs` — but the surface inside those modules is larger and differently shaped than the draft assumed.

| Module | Shape of the surface | Consequence |
|---|---|---|
| `net` | Request builder over resource handles: `init`, `set_url`, `set_header`, `set_body`, `set_timeout`, `send`, **`send_all`**, `data_len`, `read_data`, `get_image`, `get_header`, `get_status_code`, `get_url`, `html`, and **`set_rate_limit(permits, period, unit)`** | `send_all` means the host must issue concurrent requests. `set_rate_limit` means **the source declares its own rate limit**. |
| `html` | Roughly 45 functions including `select`, `select_first`, `attr`, `text`, `own_text`, `outer_html`, traversal — **and mutation**: `set_attr`, `remove_attr`, `set_text`, `set_html`, `prepend`, `append`, `add_class`, `remove_class`, `remove` | A read-only parser cannot back this module. See [correction 1](#correction-1-scraper-cannot-back-the-html-module). |
| `js` | `context_create`, `context_eval`, `context_eval_async`, `context_get` — **plus a full WebView surface**: `webview_create`, `webview_load`, `webview_load_html`, `webview_wait_for_load`, `webview_eval`, `webview_eval_async`, `webview_set_rule_list`, `webview_add_user_script`, `webview_get_cookies`, `webview_delete_cookie` | A JS engine covers only half this module. See [correction 2](#correction-2-flaresolverr-does-not-implement-the-webview-imports). |
| `canvas` | `new_context`, `set_transform`, `draw_image`, `copy_image`, `fill`, `stroke`, `draw_text`, `new_font`, `system_font`, `load_font`, `new_image`, `get_image_data`, `get_image_width/height` | A real 2D graphics surface with font loading and text rendering, not an image helper. See [correction 3](#correction-3-canvas-is-a-graphics-engine-not-a-deferrable-detail). |
| `defaults` | `get`, `set` | Two functions. Maps cleanly to the `source_kv` table as designed. |
| `std` | `destroy`, `buffer_len`, `read_buffer`, `current_date`, `utc_offset`, `parse_date`, `print`, `sleep`, **`send_partial_result`** | Confirms a host-side resource table keyed by handle. `send_partial_result` is a streaming hook that maps directly onto SSE progress events. |

Two structural facts follow from this surface. First, the ABI is **resource-handle based**: functions return an `Rid`, and `std::destroy`, `buffer_len`, and `read_buffer` manage the lifetime and transfer of host-owned objects. The host must maintain a resource table per invocation, which confirms the "one `Store` per invocation" decision in the draft. Second, values crossing the boundary are **postcard-encoded** — `aidoku` 0.3.0 depends on `postcard 1.1` with `alloc` — so the host's Rust structs must be byte-compatible with the source's, and that compatibility is what the ABI version guards.

The `Source` trait is joined by optional capability traits that a source may implement: `ListingProvider`, `Home`, `DeepLinkHandler`, `BasicLoginHandler`, `WebLoginHandler`, `ImageRequestProvider`, `CoverImageProcessor`, `PageImageProcessor`, `DynamicFilters`, `DynamicListings`, and `DynamicSettings`. `DynamicSettings` and `DynamicFilters` are what the frontend's `DynamicForm` renders.

### The reference implementation is Swift

`AidokuRunner` is a Swift package. There is no Rust host to copy, so every host import is reimplemented in Rust by reading Swift code and matching observable behavior. That is the bulk of the work this ADR commits to.

## Decision

Use Aidoku `.aix` extensions run in `wasmtime` as the primary provider mechanism, behind the existing `domain` provider ports.

### Runtime configuration

- Pin **`wasmtime = "48"`**, the current LTS. Wasmtime issues a new major version monthly; releases divisible by 12 are LTS and supported for 24 months, other releases for 2 months. Tracking `49` and later monthly majors would mean a dependency upgrade every month for a sandbox that does not need new features.
- One `Store` per invocation with a per-store resource table; a shared `Engine` with the on-disk compilation cache enabled.
- **Epoch-based interruption** for timeouts, not fuel. Fuel modifies compiled code and costs throughput; epochs are the lightweight mechanism and this project needs a wall-clock bound, not deterministic instruction counting.
- Memory cap per store in the 64–128 MB range, no WASI filesystem, no WASI sockets. Network access exists only through the `net` host import.
- `net` enforces an egress allow-list: reject `localhost`, loopback, RFC1918, link-local, and cloud metadata addresses, after DNS resolution rather than before, so a hostname cannot resolve into the private range.
- Run invocations on `spawn_blocking`, as decided in [ADR-0003](0003-run-the-job-queue-in-postgres-and-in-process.md).

### Pin the ABI by commit, not by version

Because `aidoku` is unpublished and untagged:

1. Record the exact **aidoku-rs commit SHA** the host targets in `crates/aidoku-runtime`, in code, not only in a lockfile.
2. Define the project's own `HOST_ABI_VERSION` integer. Store it alongside each installed source's compiled-module cache entry, and invalidate the cache when it changes.
3. Build a **conformance test source** — a `.aix` compiled from a fixture crate at the pinned SHA that exercises every implemented import and asserts the postcard round-trip. This is the regression test for ABI drift, and it is the only thing standing between an upstream struct change and silent misbehavior in production.
4. Re-pin deliberately. Treat an aidoku-rs SHA bump as a change that requires the conformance suite to pass, not as a dependency update.

### Implement host modules in declared tiers

Implement in this order, and record what is implemented as machine-readable capability data:

| Tier | Modules | v1 status |
|---|---|---|
| 1 | `std`, `defaults`, `net`, `html` | Required. Nothing works without these. |
| 2 | `js` context functions (`context_create`, `context_eval`, `context_eval_async`, `context_get`) via `rquickjs` | Implemented when a target source needs it. |
| 3 | `js` webview functions, `canvas` | **Not implemented in v1.** Returns a distinct unsupported-capability error. |

> [!IMPORTANT]
> Do not let an unimplemented import surface as a trap or a generic failure. `SourceRegistry` must read the required imports at install time and **refuse to install a source whose imports this host does not provide**, with a problem+json response naming the missing capability. A source that installs and then fails mid-download is the worst version of this behavior, because the failure looks like a site problem rather than a host gap.

### Corrections to the architecture draft

#### Correction 1: `scraper` cannot back the `html` module

The draft specified `scraper` for HTML parsing. `scraper` is a read-only query API over an `ego-tree` document. The `html` module requires mutation — `set_attr`, `set_text`, `set_html`, `prepend`, `append`, `remove`, `add_class`, `remove_class` — which `scraper` does not provide. Use a mutable DOM: `html5ever` (0.40.x) with an RC-DOM tree plus a selector layer, or `kuchikiki`, which bundles both. Validate the chosen option against `select`, mutation, and `outer_html` round-tripping before committing to it.

#### Correction 3: `net::get_image` is a tier-3 dependency inside a tier-1 module

The `net` row lists `get_image` alongside the request builder, which reads as tier 1. It is not: it answers a `canvas::ImageRef`, so serving it means having the image pipeline this ADR defers. It is mapped to the `canvas` capability and refused at install like any other tier-3 import. Every other function in the `net` module is implemented.

#### Correction 2: FlareSolverr does not implement the webview imports

The draft treats FlareSolverr as the Cloudflare answer. It covers one path and not the other:

- For a source that fetches through `net` and hits a challenge, FlareSolverr is a valid detour: solve, cache the clearance cookie and user-agent per domain, and replay. Keep this.
- For a source that calls `js::webview_*`, FlareSolverr is irrelevant. Those imports exist because Aidoku on iOS has a real `WKWebView`. Implementing them in Rust means embedding a browser engine — `chromiumoxide` over CDP against a headless Chrome container is the realistic option, and it is a second container plus a browser to keep patched.

So webview support is a tier-3 capability gap, declared at install time, not something FlareSolverr closes.

On FlareSolverr's own status: it is **maintained**, with v3.5.2 released 12 September 2026 against 15.6k stars. Several vendor blog posts assert it is abandoned; those come from companies selling competing services and are not supported by the repository. The real risk is narrower and documented in its own issue tracker: intermittent failures on Cloudflare managed challenges and Turnstile, where a solve returns success but the subsequent request is still blocked. Treat FlareSolverr as best-effort, surface its failures as a degraded-source state rather than a generic download error, and report reachability through `/readyz` as already designed.

#### Correction 3: `canvas` is a graphics engine, not a deferrable detail

The draft deferred `canvas` as "image ops if any source depends on them". The surface includes affine transforms, path fill and stroke, font loading from a URL, system font lookup by weight, and text drawing. Implementing it means adopting a 2D rasterizer and a text shaper. Deferring it is the right call, but it must be an explicit declared capability, because a source that descrambles page images through `canvas` will not degrade gracefully — it will produce unreadable pages.

### Honor source-declared rate limits

`net::set_rate_limit(permits, period, unit)` means sources state their own limits. Feed that value into the per-source `Semaphore` from [ADR-0003](0003-run-the-job-queue-in-postgres-and-in-process.md) instead of relying only on the 2–4 default: take the stricter of the source-declared limit and the configured cap. Ignoring a declared limit is the fastest route to an IP ban.

## Spike results

A throwaway host (`crates/aidoku-runtime/examples/spike.rs`) was built against real `.aix` packages from [Aidoku-Community/sources](https://github.com/Aidoku-Community/sources) before committing to this decision. It establishes the following as fact rather than expectation.

### The package format and the call ABI

```
ar.aasq-v2.aix
  Payload/source.json     manifest: { info: {id, name, version, url, contentRating, languages, minAppVersion}, listings?, config? }
  Payload/icon.png
  Payload/filters.json
  Payload/settings.json   optional
  Payload/main.wasm
```

Exports: `start`, `get_search_manga_list`, `get_manga_update`, `get_page_list`, `get_image_request`, `handle_deep_link`, `handle_key_migration`, `free_result`, plus `memory`.

```
get_search_manga_list(query_descriptor: i32, page: i32, filters_descriptor: i32) -> i32
  query   = raw UTF-8 bytes behind the descriptor
  filters = POSTCARD-encoded Vec<FilterValue>
```

A non-negative return is a **pointer into guest memory**, not a handle: `[0..4]` total length including the header, `[4..8]` capacity, `[8..]` postcard payload — released with `free_result(ptr)`.

### The required surface is far smaller than the ABI

> [!WARNING]
> The table below is a **sample**, not the commitment. It is what three sources happened to import; the commitment in [The v1 source commitment](#resolved-the-v1-source-commitment) is tier 1 *in full*. The host was first built to this table, which left `net::send_all` and `env::sleep` unimplemented — `send_all` is in the `net` row of [the ABI table above](#what-the-aidoku-abi-actually-requires) and is what MangaDex needs. Build against `crates/aidoku-runtime/abi/tier1-surface.txt`, which is generated from the pinned commit by `cargo xtask abi-surface` and is what the host's tests assert against.

Parsing the wasm import sections of three real sources gives **32 host functions across 5 modules**, against roughly 90 in the full ABI:

| Module | Functions needed |
|---|---|
| `env` | `abort`, `print`, `send_partial_result` |
| `std` | `abort`, `buffer_len`, `current_date`, `destroy`, `parse_date`, `print`, `read_buffer` |
| `net` | `data_len`, `html`, `init`, `read_data`, `send`, `set_body`, `set_header`, `set_rate_limit`, `set_url` |
| `html` | `attr`, `base_uri`, `get`, `html`, `own_text`, `parse_fragment`, `select`, `select_first`, `set_text`, `size`, `text` |
| `defaults` | `get`, `set` |

> [!NOTE]
> `env` was not in the module list this ADR originally gave. Those three functions land there because the corresponding `extern` block in aidoku-rs declares no explicit wasm import module. Any host that registers only the named modules will fail to instantiate.

### Two ABI landmines

Both produced silent, misattributable failures, and both are exactly what the conformance suite exists to catch:

1. **`std::read_buffer` and `net::read_data` must return `0` on success**, not the byte count. The guest does `if error != 0 { return None }`, so returning the length makes every successful read look like a failure. The visible symptom was `get_search_manga_list` returning `-1` as though the filters were malformed.
2. **`html` has its own error code space**, not `AidokuError`'s: `-1` InvalidDescriptor, `-2` InvalidString, `-3` InvalidHtml, `-4` InvalidQuery, **`-5` NoResult**, `-6` SwiftSoupError. Returning a generic error for a legitimate miss makes an absent element look like a malformed selector.

### Capability tiers, measured against the real catalog

Scanning all 136 community sources, folding in the 17 shared templates they build on:

| Requirement | Sources | Share |
|---|---|---|
| `net` | 134 | 98.5% |
| `std` | 121 | 89.0% |
| `html` | 111 | 81.6% |
| `defaults` | 30 | 22.1% |
| `canvas` (tier 3) | 22 | 16.2% |
| `js` context (tier 2) | 3 | 2.2% |
| `js` webview (tier 3) | 3 | 2.2% |

**108 of 136 sources (79%) need only tier-1 imports.** Tier 2 adds two (`zh.copymanga`, `zh.dm5`). The 24 that touch tier 3 are concentrated in Japanese and Vietnamese sources plus `multi.mangaplus` and `en.mangago`, which use `canvas::ImageRef` for page descrambling — so deferring `canvas` costs about 16% of the catalog, more than this ADR first assumed, and those sources fail by producing unreadable pages rather than degrading gracefully. The install-time capability refusal is what makes that safe.

Three templates need `canvas` (`mangareader`, `wpcomics`, `gigaviewer`), so it is not only a standalone-source concern.

### Resolved: the v1 source commitment

Follow-up 1 asked for the target `.aix` list, on the reasoning that it, rather than the full ABI, defines tier-1 and tier-2 scope. The spike inverted that: the import analysis across all 136 sources is already done, so what remained was not *which imports* but *what this project is accountable for*.

**v1 supports any source whose required imports are tier 1** — `net`, `std`, `html`, `defaults`. That is 108 of 136 community sources, 79%.

> [!IMPORTANT]
> That 79% is measured per **capability**, not per function, so it is an upper bound. A source can need only tier-1 modules and still import a function the host does not define — which is what happened to MangaDex. The install check is now function-level, so the enforcement matches the implementation rather than this figure. Re-measuring the corpus against `abi/tier1-surface.txt` would turn the bound into a number.

This is deliberately a capability commitment rather than a named list, because the install-time check already enforces exactly that boundary. A promise phrased as "these twenty sources" would say less than the host does, date on the next upstream release, and need a code change to stay current. Phrased as a capability, the promise and the enforcement are the same mechanism, and a tier-1 source published upstream tomorrow works with no change here.

Seven sources are the named regression set, chosen because the spike proved them end to end rather than because they are the most popular:

| Source | Shape | Entries |
|---|---|---|
| `en.dankefurslesen` | JSON API | 20 |
| `en.hivescans` | JSON API | 18 |
| `en.guya` | JSON API | 6 |
| `ar.aasq` | HTML scrape | 21 |
| `en.asurascans` | HTML scrape | 20 |
| `en.flamecomics` | HTML scrape | 167 |
| `en.weebcentral` | HTML scrape | 31 |

Both shapes are represented on purpose. The `abs:` bug was invisible in JSON sources and fatal in every HTML one, so a regression set of only the first kind would have passed straight through it.

#### What is deferred, and how it fails

| Capability | Sources | v1 |
|---|---|---|
| `js` context | 3 (2.2%) | Feature-gated behind `js`, off by default |
| `canvas` | 22 (16.2%) | Not implemented |
| `js` webview | 3 (2.2%) | Not implemented |

`canvas` is the expensive omission, and the decision is to take the cost visibly. Those sources use `canvas::ImageRef` to descramble page images, so without it they would produce unreadable pages — but the install-time check refuses them outright, naming the capability. A user sees *this build does not provide canvas* before installing, rather than a series that installs and then renders scrambled. This ADR already identifies the second outcome as the worst one available, and refusing at install is what the check was built for.

The concentration matters more than the percentage: the 22 are mostly Japanese and Vietnamese sources, plus `multi.mangaplus` and `en.mangago`, and three shared templates (`mangareader`, `wpcomics`, `gigaviewer`). Deferring `canvas` is therefore a decision about which languages v1 serves well, not an even 16% haircut. Revisit on demand rather than on principle.

`js` context stays behind a cargo feature because enabling it puts a script engine in every deployment so that two sources work, and a script engine is a real widening of what a hostile package can attempt. The two are `zh.copymanga` and `zh.dm5`.

#### Verification: fixture per commit, live on a schedule

The conformance `.aix` is the per-commit gate and runs with no network. The seven real sources run on a schedule, report-only.

That split follows from this ADR's own negative result. A host cannot be developed against live sites, because a failure there does not distinguish *the host regressed* from *the site changed its markup* — and this ADR expects the second to happen routinely. Making a live run block a merge would import someone else's deployment schedule into this project's; making it report instead turns markup drift into an issue to triage, which is what it is.

### Source-declared rate limits are real

`en.asurascans` called `net::set_rate_limit(2, 2, 0)` during `start`. This confirms the interaction with ADR-0003: take the stricter of the declared limit and the configured cap.

### What works, and what remains open

Working end to end, with correct data: **JSON-API sources.** `en.dankefurslesen` returned 20 entries (4,057-byte payload) in 13 host calls, `en.hivescans` 18 entries in 11 calls, `en.guya` 6 entries in 13 calls. Every import they needed was satisfied; none reached a stub.

**Not working: HTML-scraping sources.** `ar.aasq`, `en.asurascans`, `en.flamecomics`, and `en.weebcentral` all instantiate, fetch, parse, make hundreds to thousands of `html::*` calls, and return a well-formed result containing **zero entries**. The spike's `html` module represents nodes as re-parsed outer-HTML strings, which is adequate for a simple fixture — a selftest confirms `select` → `get` → `attr` → nested `select_first` all return correct values — but does not reproduce Jsoup's behaviour on real pages. Correcting two semantics (a missing attribute must be `NoResult` rather than an empty string; `select` must exclude the element itself) reduced the call volume substantially without producing entries.

> [!IMPORTANT]
> This was the spike's most useful negative result: **a host implementation cannot be developed against live sites.** With a live page you cannot distinguish "my `html` module is wrong" from "the site changed its markup" — and this ADR already expects the latter to happen routinely. The conformance `.aix` with **fixed fixture HTML** is therefore not a regression test to add afterwards; it is the only way to build the `html` module at all. Follow-up 3 moves ahead of the host implementation work, not after it.

### Resolved: the `abs:` attribute prefix

The conformance fixture found the cause within one iteration, and it was not a DOM-fidelity problem at all.

Aidoku follows **Jsoup's `abs:` convention**: `element.attr("abs:src")` means *resolve this attribute value against the document's base URI*. Sources use it for every cover image and series link. A host that treats `abs:href` as a literal attribute name finds nothing, returns a miss, and the source discards the entry — silently, with no error anywhere. That is why HTML-scraping sources produced well-formed results containing zero entries while JSON sources were unaffected.

Three things are required, and all three are now covered by fixture checks:

1. **`html::attr` must honour the `abs:` prefix**, stripping it and resolving the raw value against the base. A plain lookup must still return the unresolved value.
2. **`html::base_uri` must return the document base**, which arrives either from the base-URL argument to `parse`/`parse_fragment` or, far more commonly, from the URL of the request that `net::html` was called on. Threading the request URL into the document is not optional.
3. **The base must propagate** through `select` → `get` → nested `select_first`, because that is the path by which a source reaches the `<img>` inside a card.

With those implemented, every source tested returns real data, including absolute cover URLs:

| Source | Before | After |
|---|---|---|
| `ar.aasq` | 0 entries | **21** |
| `en.asurascans` | 0 entries | **20** |
| `en.flamecomics` | 0 entries | **167** |
| `en.weebcentral` | 0 entries | **31** |
| `en.dankefurslesen` | 20 | 20 |
| `en.hivescans` | 18 | 18 |
| `en.guya` | 6 | 6 |

The conformance suite stands at 22 checks, all passing. Two of them exist only because they were wrong first: the `abs:` resolution checks, and the discovery that an empty string and an absent value are indistinguishable across this boundary.

This is the strongest available evidence for follow-up 3's ordering. The bug was invisible against live sites — four sources failed identically for a reason that looked like markup drift — and obvious against a fixture that asserted one behaviour by name.

`html::set_text` was never reached on the listing path by any of the seven sources. Mutation is still in the required import surface, so it must be implemented, but it is not on the critical path for catalog browsing — which lowers the urgency of the mutable-DOM choice in follow-up 4 without removing it.

It is also worth recording what the spike did **not** find. The throwaway `html` module models nodes as re-parsed outer-HTML strings, which is inefficient and loses parent and sibling context, and it was the prime suspect for the zero-entry failure. It was not the cause: all 22 conformance checks pass against it, including `own_text` excluding child elements and `select` excluding the element itself. The real implementation should still keep a tree with node ids — `parent`, `siblings`, `next`, and `prev` are in the ABI and cannot be served by a detached string — but the string model was adequate for the listing path, and the DOM choice is a performance and completeness decision rather than a correctness blocker for browsing.

## Consequences

### What you gain

- **Sandboxing is the default, not an add-on.** Untrusted provider code gets a linear memory, an epoch deadline, and exactly the imports you wrote. There is no ambient filesystem, no socket API, and no path to the database. No other provider mechanism considered here starts from that position.
- **Providers update without a deployment.** Installing a `.aix` is a database write plus a module compile. Source updates are a job kind (`update_sources`), not a release.
- **You inherit an ecosystem instead of writing scrapers.** This is the entire point, and it is what makes the host-implementation cost worth paying.
- **`send_partial_result` maps onto the existing SSE design.** Sources can stream partial results, and the host can forward them as `job.progress` events without inventing a mechanism.
- **Deep observability is cheap.** `aidoku.call` spans carry `source.id`, `source.version`, `aidoku.fn`, `wasm.duration_ms`, and `wasm.memory_peak_bytes`, because the host controls every entry and exit point.

### What it costs you

- **This is the largest build item in the project.** The `html` module alone is roughly 45 functions, `net` is a stateful request builder with concurrency, and `std` requires a resource table with correct handle lifetimes. Budget accordingly, and expect the conformance suite to be as large as the implementation.
- **No semver protection.** An upstream change to a postcard-encoded struct is a silent wire-format break. The pinned SHA, the `HOST_ABI_VERSION`, and the conformance source are the only defenses. If any of the three is skipped, this decision becomes unsafe.
- **The reference is in another language.** Behavior parity is established by reading Swift and testing against real sources, not by reusing code.
- **v1 has declared capability gaps.** Sources requiring `js::webview_*` or `canvas` cannot be installed. That is a product limitation with a visible error message, which is the best available outcome, but it is still a limitation.
- **Cloudflare handling is best-effort and will break periodically.** Not because FlareSolverr is unmaintained, but because challenge evasion is adversarial by nature.
- **Monthly wasmtime majors.** Pinning LTS reduces this to a deliberate upgrade every 12 releases, with security backports guaranteed in between, but the 24-month LTS clock is a real calendar item.
- **Legal and ToS exposure is inherited.** The application fetches from sites that generally do not license this access. That is a property of the product, not of this decision, but the provider layer is where it is concentrated.

### Follow-up work this decision creates

1. ~~**Pick the target sources first.**~~ **Resolved: the commitment is capability-defined.** See [Resolved: the v1 source commitment](#resolved-the-v1-source-commitment). The spike's import analysis across all 136 sources made a named list the wrong shape — the install check already enforces the tier boundary, so the promise and the enforcement are one mechanism.
2. **Record the aidoku-rs SHA and add `HOST_ABI_VERSION`** to the module cache key, in the first commit that touches the runtime.
3. **Build the conformance `.aix`** and wire it into CI.
4. ~~**Choose and validate the mutable DOM.**~~ **Resolved: `dom_query`.**

   | Candidate | Verdict |
   |---|---|
   | **`dom_query` 0.28** | **Chosen.** `html5ever` 0.39, `selectors` 0.38, `cssparser` 0.37 — one minor behind current and internally coherent. |
   | `kuchikiki` 0.8.2 | Rejected. `html5ever` 0.26, `selectors` 0.22, and `indexmap` 1.x: years behind, and it would duplicate `html5ever` against anything current. |
   | `html5ever` + RC-DOM + `selectors` | Rejected. Assembling selector matching, mutation, and serialization by hand is most of `dom_query`. |
   | `scraper` | Rejected. Read-only, so it cannot back `set_attr`, `set_text`, `set_html`, `append`, `prepend`, or `remove`. |

   `dom_query` maps onto the ABI almost directly: `NodeId` and `NodeRef` give the stable, lifetime-free handles the resource table needs; `select` and `select_single` cover `select` and `select_first`; `text` and `immediate_text` are `text` and `own_text`; `html` and `inner_html` cover serialization; and it implements **`base_uri()` on both document and node** — the behaviour whose absence made every HTML-scraping source return zero entries.

   Two things to verify while implementing, since they are not in its documented surface: sibling traversal (`next`, `prev`, which the ABI requires) and class helpers (`add_class`, `remove_class`, `has_class`, which `set_attr("class", …)` can back).
5. **Implement install-time capability negotiation** in `SourceRegistry`, with a problem+json error naming missing capabilities, plus a `GET /sources/{id}` field exposing which capabilities a source needs.
6. **Wire `set_rate_limit` into the job engine's semaphore** and log when a source-declared limit is stricter than the configured cap.
7. **Fuzz the postcard decode path.** It parses attacker-influenced bytes from a source into host structs. `cargo-fuzz` on the decoder is proportionate to that exposure.
8. **Add the egress allow-list test** covering DNS rebinding: a hostname resolving to `169.254.169.254` or an RFC1918 address must be rejected after resolution.

## Alternatives considered

| Alternative | Ecosystem size | Sandbox | Outcome |
|---|---|---|---|
| Aidoku `.aix` on wasmtime | Moderate | Strong, by construction | **Chosen** |
| Mihon / Tachiyomi Kotlin extensions | Largest by a wide margin | None; full JVM privileges | Rejected. Requires a JVM sidecar and runs untrusted code unsandboxed. |
| Paperback JS sources | Moderate, iOS-centric | Weak to moderate | Rejected. Weaker isolation, and the JS engine work is a subset of what Aidoku needs anyway. |
| Native Rust adapters per site | Zero; you are the ecosystem | Not needed | Rejected as the primary mechanism; retained as an escape hatch. |
| A bespoke WASM plugin ABI | Zero | Strong | Rejected. All the host cost, none of the ecosystem. |

### Mihon / Tachiyomi Kotlin extensions

This is the strongest alternative on the criterion that matters most — ecosystem size. The Mihon extension ecosystem is substantially larger and better maintained than Aidoku's, and Suwayomi-Server demonstrates that running those extensions on a server is practical.

It was rejected on two grounds:

1. **No sandbox.** Kotlin extensions are JVM code loaded as APK-packaged classes. They run with the privileges of the host process: full filesystem access, arbitrary sockets, and reachability to Postgres. The requirement that provider code be untrusted is not satisfiable in that model without a separate hardened container per extension, which costs more than implementing the Aidoku ABI.
2. **A JVM in the deployment.** A single-binary Rust service becomes a Rust service plus a JVM sidecar, its heap, its startup time, and its own update path — on the same single VM as `postgres` and `flaresolverr`.

The ecosystem advantage is real and this is the alternative to reconsider first if the Aidoku source catalog proves too thin for the target sites.

### Paperback JS sources

JavaScript sources would run in `rquickjs` or a V8 binding. Rejected because the isolation story is weaker than WASM linear memory with epoch deadlines, the ecosystem is oriented toward iOS, and the effort overlaps: the `js` tier-2 work for Aidoku already requires embedding a JS engine, so this offers no saving.

### Native Rust adapters per site

Full control, no host ABI, no sandbox needed because the code is yours, and compile-time type safety end to end. Rejected as the primary mechanism because the maintenance load is unbounded and adversarial — every site redesign is your bug, and the `Source` port in `domain` exists precisely so a native adapter can be added for one high-value site without reopening this decision.

### A bespoke WASM plugin ABI

Designing a cleaner ABI than Aidoku's is achievable, and the sandboxing benefits are identical. Rejected because it delivers zero providers on day one. The host implementation is the cost; the ecosystem is the benefit. Keeping the cost and discarding the benefit inverts the trade.

## Revisit this decision when

- **The target source list needs capabilities in tier 3.** That is the trigger to price a headless-Chrome container for `webview_*`, or a rasterizer and text shaper for `canvas`.
- **The Aidoku catalog does not cover the sites you need.** Reopen the Mihon comparison, and price a per-extension sandbox container against the remaining Aidoku host work.
- **The conformance suite breaks on an aidoku-rs SHA bump in a way that is not mechanical.** Repeated wire-format churn upstream would make the unpublished-crate risk the dominant cost.
- **FlareSolverr's failure rate makes a meaningful share of sources unusable.** The alternative is a CDP browser, which is the same container that would unlock `webview_*` — so evaluate both together.
- **Wasmtime 48's LTS window nears its end**, roughly 24 months from its release, at which point the next LTS (60) is the target.

## References

- [`aidoku-rs` repository](https://github.com/Aidoku/aidoku-rs) — host import modules under `crates/lib/src/imports/`
- [`aidoku` crate API documentation](https://aidoku.github.io/aidoku-rs/aidoku/index.html) — version 0.3.0, `Source` and optional capability traits
- [Aidoku source development book](https://aidoku.github.io/aidoku-rs/book/)
- [`AidokuRunner`](https://github.com/Aidoku/AidokuRunner) — Swift reference runner
- [Wasmtime release process and LTS policy](https://docs.wasmtime.dev/stability-release.html)
- [Wasmtime LTS RFC](https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasmtime-lts.md)
- [Wasmtime: interrupting execution with epochs](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
- [`wasmtime::Config`](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html)
- [FlareSolverr repository](https://github.com/FlareSolverr/FlareSolverr) — v3.5.2, 12 September 2026
- [FlareSolverr issue 1664: solve returns 200 but the request is still blocked](https://github.com/FlareSolverr/FlareSolverr/issues/1664)
- [FlareSolverr issue 1734: challenge not detected on some sites](https://github.com/FlareSolverr/FlareSolverr/issues/1734)
- [`html5ever`](https://crates.io/crates/html5ever) and [`kuchikiki`](https://crates.io/crates/kuchikiki) — mutable DOM candidates
- [`chromiumoxide`](https://crates.io/crates/chromiumoxide) — CDP client, if webview support is ever priced in
