# ADR-0021: Separate the server's data directory from the library

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-23 |
| **Deciders** | eddy.castillo@mobiik.com |
| **Applies to** | `crates/api/src/config.rs`, `crates/packaging/src/packages.rs`, `crates/aidoku-runtime`, deployment |
| **Supersedes** | None |

The server keeps its own persistent state under `DATA_DIR`, which is required, must not overlap `LIBRARY_ROOT`, and today holds the installed source packages. This article explains what forced a second location, why nesting it inside the library was rejected, and what an operator now has to provision.

## Context

### A source was unusable after any restart

A source is a WASM module. [ADR-0004](0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md) has the server download an `.aix`, verify its capabilities, compile it, and hold the result in memory. The package bytes were then discarded.

Nothing reloaded them. `SourceRegistry::install` was the only path that populated the runtime's map, so after a restart every installed source was still listed by `GET /sources` and returned 404 from `/catalog`, `/filters` and `/settings`. Browse, downloads and follow checks were all dead until an operator reinstalled each source from the network.

For a self-hosted server that restarts on every upgrade, container recreation and reboot, that is the whole application failing on a routine event.

> [!NOTE]
> This was found while building the settings editor, by installing a real source and restarting. No test caught it: the live-source tests install and use within one process, and the API tests use a fake registry. Both are reasonable tests that share an assumption — that the process which installs is the process which reads.

### The bytes have to go somewhere that is not the library

[ADR-0007](0007-store-the-library-as-cbz-files-on-a-local-volume.md) commits the library to plain CBZ files on a volume, so that Komga, Kavita and offline readers can open it. That is what makes the library something an operator backs up, restores and syncs between machines.

Those three operations are the problem. Server state nested inside the library would be carried by every one of them: a backup would include third-party executable code, and restoring last month's library would silently roll back which sources are installed along with it.

### Compiled modules are not portable anyway

A `wasmtime` module compiled by one version is not loadable by another, so nothing here can be a cache that travels. The package bytes are the durable artifact, and compilation happens at every boot.

## Decision

`DATA_DIR` is a required environment variable naming a directory the server owns.

- **Required, not defaulted.** A default would put server state somewhere an operator did not choose and would not think to back up or exclude.
- **Refused when it overlaps `LIBRARY_ROOT`.** Either containing the other fails the boot with both paths named. Compared lexically after normalising, with a trailing separator — `/srv/lib` and `/srv/library` are a perfectly good pair and must not be refused.
- **Packages live at `DATA_DIR/sources/{source_id}.aix`**, named by the uuid this server assigned. A source's `external_id` comes from a third-party package and would need sanitising; a uuid cannot escape its directory however it was obtained.
- **Written through a temporary file and a rename**, in the same directory, because `rename` is only atomic within a filesystem and `DATA_DIR` may be its own mount. Partial writes are swept at startup.
- **Every installed source is compiled at startup**, not on first use. One that fails is logged and skipped.

> [!IMPORTANT]
> A package that will not load must not stop the boot. Failing there takes a whole library offline over one source, and an operator cannot fix what will not start. The affected source reports that it is installed but not loaded, and says to reinstall.

## Consequences

### What you gain

- A restart is an ordinary event. Sources survive it without reaching the network.
- A source that cannot load says which one and why, at boot, where an operator is watching — the same reasoning that discovers the OIDC issuer at startup ([ADR-0005](0005-act-as-the-oidc-client-with-server-side-sessions.md) follow-up 5).
- The library stays exactly what ADR-0007 promised: comics and nothing else.
- An unloaded source is now distinguishable from a nonexistent one. Both previously reported "no source matches that identifier", which sent an operator looking for a wrong id.

### What it costs you

| Cost | Where it lands |
|---|---|
| A second volume to provision, mount and document | Compose file, Dockerfile, README, deployment runbook |
| A required variable that breaks an existing deployment until set | Anyone upgrading past this change |
| Compile time at boot, proportional to the number of installed sources | Startup, roughly 50–200 ms per source |
| Disk under `DATA_DIR`, one package per source | ~100 KB–2 MB each |
| Sources installed before this change have no package | Reported at boot; fixed by reinstalling |

The boot cost is the one that grows. At twenty sources it is a second or two; at a hundred it would be noticeable, and lazy compilation on first use becomes the better trade.

### Follow-up work this decision creates

1. The Compose file and Dockerfile declare a `DATA_DIR` volume, and the deployment runbook says it is not the one to restore from a library backup.
2. A regression test that a package saved by one runtime loads into a fresh one — the restart, without a process boundary. `crates/aidoku-runtime/tests/restart.rs`.
3. A test that `install` registers under the id the database returned, not one generated locally. The bug it guards is invisible from either side of the download, which is why `install_bytes` exists as a seam.
4. Revisit eager compilation when a deployment has enough sources for boot time to matter, or when a `wasmtime` upgrade makes a serialized module cache worth keeping under `DATA_DIR`.

## Alternatives considered

| Alternative | Outcome |
|---|---|
| A subdirectory of `LIBRARY_ROOT` | Rejected. Backups, restores and syncs would carry it. |
| A `bytea` column on `source` | Rejected. Puts megabytes of blobs in a row the API reads constantly. |
| Re-download at startup | Rejected. Makes booting depend on the network and on a third party. |
| Compile lazily, on first use | Rejected for now. Hides failures until a reader hits one. |

### A `bytea` column on `source`

This is the strongest alternative, and it wins on operational simplicity: one thing to back up, one thing to restore, no second volume, no new variable, and the package cannot drift from the row that describes it. A restore is atomic in a way two volumes never are.

It was rejected on read cost. `GET /sources` returns every installed source and is called on the library screen, the browse screen and the settings screen; `SourceRepository::list` runs at startup to register rate limits. Putting a megabyte of package bytes on that row means either fetching them constantly or maintaining a projection that excludes them — and the second is the same split as this decision, with a worse storage medium.

The drift objection is real and is answered rather than dismissed: a source row whose package is absent is reported at boot and says to reinstall, which is a state the system already had to handle for anything installed before this change.

### Compiling lazily, on first use

Cheaper boots, and no work for sources nobody browses. Rejected because it moves a failure from a log line an operator reads at startup to a 502 a reader gets mid-search, and because the failure modes here — a missing package, a `wasmtime` version change — are exactly the ones worth knowing about before serving traffic. Revisit when boot time makes the trade worth reversing.

## Revisit this decision when

- Boot time becomes noticeable, which means enough sources that eager compilation is the wrong default.
- `DATA_DIR` accumulates a second kind of state. One directory holding packages is not a layout decision; one holding packages, caches and exports is.
- The deployment target has no second writable volume. That would force the `bytea` column, and the read-cost objection above is what would need answering.

## References

- [ADR-0004: Use Aidoku WASM sources as the provider mechanism](0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md) — what a package is and why it is verified at install
- [ADR-0007: Store the library as CBZ files on a local volume](0007-store-the-library-as-cbz-files-on-a-local-volume.md) — the library this must not be nested in
- [wasmtime: `Module::deserialize`](https://docs.rs/wasmtime/latest/wasmtime/struct.Module.html#method.deserialize) — why a compiled module is not a portable artifact
