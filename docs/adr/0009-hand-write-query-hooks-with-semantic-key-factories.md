# ADR-0009: Hand-write Query hooks with semantic key factories

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `web/src/features/*/api`, `web/src/shared/api` |
| **Supersedes** | None |

This article explains why TanStack Query hooks are written by hand over a generated fetch client, rather than generated from the OpenAPI document by Orval, Hey API, or `openapi-react-query`. The reason is narrower than "generated hooks hide the cache", which is not true of the current tools — it is about what a query key means.

> [!NOTE]
> Research for this ADR weakened the rationale in the architecture draft. `openapi-react-query` exposes its query keys, supports infinite queries, and defaults its page parameter to `cursor`, which matches this API's pagination convention exactly. It is a much closer call than the draft implies, and the decision below rests on a different argument.

## Context

Types are already generated and that is not in question: `cargo xtask openapi` writes `web/openapi.json`, `openapi-typescript` 7.13.0 turns it into `schema.d.ts`, and `openapi-fetch` 0.17.0 provides the typed request function. CI fails when either is stale. The question this ADR settles is whether the **TanStack Query layer** above that client is also generated.

### Query keys are the integration point for live updates

Per the frontend design and ADR-0010, a single `EventSource` drives every cache update in the application. The handler table is written in terms of resources, not endpoints:

| Event | Cache action |
|---|---|
| `job.progress` | Patch `jobsKeys.detail(id)`; update that row inside the infinite list in place |
| `job.state` | Patch, then invalidate `jobsKeys.list()` counts; toast on `failed` |
| `chapter.new` | Invalidate `libraryKeys.chapters(mangaId)` **and** the follows badge |
| `chapter.downloaded` | Patch the chapter row, invalidate storage stats |
| `source.updated` | Invalidate `sourcesKeys.all` |

Two of these cut across endpoints. `chapter.new` touches a chapter list and a follows count, which are different paths. `sourcesKeys.all` means "everything about sources", which spans `GET /sources`, `GET /sources/{id}/settings`, and `GET /sources/{id}/filters`. So the cache needs to be addressable by **resource**, not only by URL.

### Several behaviors are hand-written regardless of the generator

- **Optimistic updates with rollback** for follow toggles and job cancel.
- **`Idempotency-Key` generated once per `POST /downloads` submit and reused on retry**, which requires the key to outlive a single mutation attempt.
- **Patching a row inside an infinite list**, which means walking `pages[]` and replacing an entry.
- **Per-resource `staleTime`**: catalog 5 minutes, library 30 seconds, jobs 0 because SSE drives them.

### The spec producer emits OpenAPI 3.1 nullable patterns

`utoipa` 5.x generates OpenAPI 3.1, where nullability is expressed as a type array or as `oneOf: [{type: "null"}, {$ref: ...}]` rather than `nullable: true`. This is correct 3.1, and it is also a documented friction point for some generators — Orval has open issues around the nullable-`$ref` pattern, including MSW mock output missing imports for its aliased types. `openapi-typescript` handles 3.1 as its primary target, which is one reason the type layer stays where it is.

## Decision

Write the Query layer by hand, on top of the generated `openapi-fetch` client.

- **One `api/` module per feature**, colocating hooks with their key factory: `features/jobs/api/`, `features/sources/api/`, and so on.
- **Semantic key factories**, hierarchical and resource-oriented:

  ```ts
  export const sourcesKeys = {
    all:            ['sources'] as const,
    lists:      () => [...sourcesKeys.all, 'list'] as const,
    detail: (id: string) => [...sourcesKeys.all, 'detail', id] as const,
    catalog: (id: string, params: CatalogSearch) =>
                   [...sourcesKeys.all, 'catalog', id, params] as const,
  }
  ```

  Every key starts with the resource, so `invalidateQueries({ queryKey: sourcesKeys.all })` reaches every cached query about sources through TanStack Query's prefix matching, regardless of which endpoint produced it.
- **Search parameters map into keys deterministically.** The validated search object from `validateSearch` (ADR-0008) is the last key segment, so a filtered view has exactly one cache entry and no accidental duplicates from key-ordering differences.
- **All requests go through the single `openapi-fetch` client** with its middleware: `X-Requested-With`, problem+json to `ApiError`, `401` to the login redirect, `Retry-After` on 429 and 503. Hooks never call `fetch` directly.
- **Keep the generated pieces generated.** `openapi-typescript` for types, `openapi-fetch` for the request function, `openapi-msw` for typed test handlers. Nothing here argues for hand-writing those.

> [!IMPORTANT]
> The key factory is the public interface of a feature's data layer. SSE handlers, route loaders, and components must all address the cache through it, never with an inline array literal. An inline key that differs by one segment creates a second cache entry that live updates will never reach, and the symptom appears later as a view that silently stops refreshing.

## Consequences

### What you gain

- **Resource-oriented invalidation.** "Everything about this source" or "this manga's chapters and its follow badge" is one prefix or two explicit keys. With path-based keys, `["get", "/manga/{id}"]` and `["get", "/manga/{id}/chapters"]` share only the method segment, so the same intent requires enumerating endpoints — and stays correct only until someone adds a third.
- **The SSE handler table is directly implementable.** It is already written in terms of `jobsKeys.detail(id)` and `libraryKeys.chapters(mangaId)`; the key factories are those names.
- **Per-resource cache policy lives with the resource.** `staleTime`, `gcTime`, retry behavior, and the `select` shape sit next to the hook they belong to instead of in generator configuration.
- **The hand-written parts are not fighting a generator.** Optimistic updates, infinite-list row patching, and idempotency-key reuse are ordinary code in the same file as the hook, rather than wrappers around generated output.
- **One dependency fewer in the data path.** `openapi-react-query` is at 0.5.4, pre-1.0, last published February 2026. Avoiding it is not a judgment on its quality; it is one less pre-1.0 package between the app and its server state.

### What it costs you

- **Boilerplate, proportional to the API surface.** The backend exposes roughly forty operations across nine resource groups. Each needs a hook, and a generator would have guaranteed coverage for free. This is the real price of the decision.
- **Coverage is a discipline, not a guarantee.** A new endpoint does not produce a hook automatically. Nothing fails if one is missing; it simply does not exist, and someone calls the client directly. The `knip` dead-export check will not catch that either.
- **Type safety stops at the call.** `openapi-fetch` types the request and response, but the mapping from a path to a key factory is conventional. A hook could pair the wrong key with the wrong endpoint and compile.
- **A second place to keep in step with the spec.** When an endpoint gains a query parameter, the generated types change automatically and the hook does not. The compiler catches most of this because the params object is typed, but not a parameter that is optional.

### Follow-up work this decision creates

1. **Write one feature's `api/` module as the reference** — `jobs`, since it exercises detail, infinite list, optimistic cancel, and SSE patching — and make it the pattern the others copy.
2. **Add a lint rule banning inline query-key arrays** outside `features/*/api/`, enforcing the invariant in the callout above. This is the single highest-value guardrail here.
3. **Test the coverage gap deliberately**: a test that enumerates operations in `openapi.json` and asserts each has a corresponding hook or an explicit exemption. This restores the one guarantee generation would have provided.
4. **Centralize the key-from-search-params helper** so every feature serializes the validated search object into a key the same way.
5. **Document the `Idempotency-Key` lifecycle**: generated once per submit in the mutation hook, held across retries, discarded on success.

## Alternatives considered

| Alternative | Latest (Sept 2026) | Key shape | Outcome |
|---|---|---|---|
| Hand-written hooks over `openapi-fetch` | — | Semantic, resource-first | **Chosen** |
| `openapi-react-query` | 0.5.4, Feb 2026 | `[method, path, params]` | Rejected, narrowly. Path-based keys do not group by resource. |
| Hey API TanStack Query plugin | 0.99.0, Jun 2026 | Generated `…Options()` factories | Rejected. Same key-shape issue, larger generated surface. |
| Orval | 8.36.0, Sept 2026 | Generated hooks per operation | Rejected. Hook-centric output plus known 3.1 nullable-`$ref` defects. |
| Kubb | 5.3.11, Sept 2026 | Plugin-configurable | Rejected. Most configurable, most configuration to own. |

### `openapi-react-query`

This is the closest alternative and it is better than the architecture draft credits. It is a roughly 1 kB wrapper over TanStack Query, it does **not** hide the cache, and its behavior lines up with this project in two specific ways:

- `queryOptions("get", "/users/{user_id}", { params: { path: { user_id: 5 } } })` produces the key `["get", "/users/{user_id}", { path: { user_id: 5 } }]`, which is directly usable with `invalidateQueries` and `setQueryData`.
- `useInfiniteQuery` takes `getNextPageParam` and `initialPageParam`, and its `pageParamName` **defaults to `"cursor"`** — the exact convention this backend uses.

So the draft's stated reason for rejecting generated hooks does not hold against this library. The reason it is still not chosen is the shape of the key rather than its visibility:

1. **Keys are path-shaped, so they do not group by resource.** The SSE contract needs "everything about sources" and "this manga's chapters plus the follows badge". Those are semantic groupings that span endpoints, and a `[method, path, params]` key can only be prefix-matched by method and path. Each such invalidation becomes an enumeration of endpoints that must be revisited whenever an endpoint is added.
2. **The third key segment is a structural object.** Exact-match `setQueryData` requires reconstructing that object precisely. For SSE patches triggered by an event that carries only an ID, the handler has to know the full params shape of the query it is patching, including query parameters it had no part in choosing.
3. **It does not remove the hand-written work.** Optimistic rollback, infinite-list row patching, and idempotency-key reuse remain the same code either way, so the saving is the thin part and the cost is the key model.

If hook boilerplate becomes a genuine maintenance problem, this is the alternative to adopt — and the migration cost is concentrated in one place: rewriting the key factories and every SSE handler that uses them.

### Hey API TanStack Query plugin

Hey API generates options factories rather than hooks — `getPetByIdOptions()` — which is the more modern pattern and composes well with TanStack Query v5. It was rejected for the same key-shape reason as `openapi-react-query`, with a larger generated surface to review and a faster-moving pre-1.0 version line at 0.99.0.

### Orval

Orval remains hook-centric by default, generating `useListPets()` and `useGetPetById()` per operation, and its distinguishing feature is built-in mock generation. Two things ruled it out: the generated hooks own the key structure most firmly of the options considered, and its handling of the OpenAPI 3.1 nullable-`$ref` pattern has open defects — including MSW output that omits imports for `__`-aliased nullable types. Since `utoipa` emits exactly that pattern for optional references, this is a live risk rather than a theoretical one. Mock generation is also already covered by `openapi-msw` against the same schema.

### Kubb

The most configurable of the generators, with a plugin per concern. Rejected because configurability is the cost here: the project would own a generator configuration whose output still needs the same semantic key layer on top.

## Revisit this decision when

- **Hook boilerplate measurably slows feature work**, or an endpoint ships without a hook and reaches production. Adopt `openapi-react-query` and accept path-shaped keys, restructuring the SSE handlers accordingly.
- **The SSE contract simplifies** to per-endpoint invalidation with no cross-resource fan-out. The main argument for semantic keys would disappear.
- **`openapi-react-query` reaches 1.0 with configurable key construction.** A resource-first key option would remove the objection entirely and make the generated path clearly better.
- **The API surface grows well past forty operations.** At some scale, guaranteed coverage beats semantic keys, and the trade flips.

## References

- [`openapi-typescript` and `openapi-fetch`](https://openapi-ts.dev/) — 7.13.0 and 0.17.0
- [`openapi-react-query`](https://openapi-ts.dev/openapi-react-query/) — 0.5.4
- [`openapi-react-query`: queryOptions and key structure](https://openapi-ts.dev/openapi-react-query/query-options)
- [`openapi-react-query`: useInfiniteQuery](https://openapi-ts.dev/openapi-react-query/use-infinite-query)
- [Hey API: TanStack Query v5 plugin](https://heyapi.dev/docs/openapi/typescript/plugins/tanstack-query)
- [Orval: React Query guide](https://orval.dev/docs/guides/react-query/)
- [Orval issue 1817: nullable properties in OpenAPI 3.1](https://github.com/orval-labs/orval/issues/1817)
- [Orval issue 3269: MSW output missing imports for nullable `$ref` aliases](https://github.com/orval-labs/orval/issues/3269)
- [utoipa issue 1215: query params should not be nullable](https://github.com/juhaku/utoipa/issues/1215)
- [TanStack Query: query options](https://tanstack.com/query/latest/docs/framework/react/guides/query-options)
- [Kubb: comparison with Orval, Hey API, and openapi-typescript](https://kubb.dev/docs/5.x/guide/comparison)
