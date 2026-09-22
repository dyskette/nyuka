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

1. **Pick the target sources first.** Choose the specific `.aix` sources v1 must support, then read their required imports. That list, not the full ABI, defines the tier-1 and tier-2 scope. Doing this before writing host code is the single highest-leverage step in the project.
2. **Record the aidoku-rs SHA and add `HOST_ABI_VERSION`** to the module cache key, in the first commit that touches the runtime.
3. **Build the conformance `.aix`** and wire it into CI.
4. **Choose and validate the mutable DOM** against `select`, mutation, and `outer_html` round-trips.
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
