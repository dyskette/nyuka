# ADR-0007: Store the library as CBZ files on a local volume

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-09-21 |
| **Deciders** | dyskette |
| **Applies to** | `crates/packaging`, `crates/domain` (`LibraryStore` port) |
| **Supersedes** | None |

This article explains why downloaded chapters are written as CBZ archives with an embedded `ComicInfo.xml` onto a local volume, rather than to object storage or as loose image files. It also resolves two questions the architecture draft left open: the on-disk naming layout, and whether the library has one root or several.

## Context

The application downloads chapter pages and stores them so the user can read them later — both through this application and through other software.

### Interoperability is the reason the library exists on a filesystem

A downloaded library that only this application can read is worth much less than one the user's other tools can also open. CBZ — a ZIP archive of page images in filename order — is the format every relevant reader supports: Komga, Kavita, YACReader, Calibre, CDisplayEx, and the local-source readers in the Mihon lineage. `ComicInfo.xml` embedded inside the archive is the metadata channel those tools read, and it is the reliable alternative to having them guess series and chapter numbers from filenames.

The format has a governance story now. It originated with ComicRack, and the [Anansi Project](https://anansi-project.github.io/docs/comicinfo/intro) maintains the schema. **Version 2.0 is the released schema; version 2.1 is a draft.**

The `Manga` field matters specifically here: its enum is `Unknown | No | Yes | YesAndRightToLeft`, which is how reading direction reaches other readers.

### Server-side reads also come from these files

`GET /api/v1/downloads/{chapter_id}/file` streams the CBZ to the browser. The `downloaded_chapter` entity records path, size, checksum, and packaging time, and the metrics snapshot reports `disk.library_free_bytes`. So the store is both an interoperability artifact and the application's own read path.

### Writes are untrusted at the edges

Series titles, chapter titles, and page filenames originate from third-party sources running in the WASM sandbox described in [ADR-0004](0004-use-aidoku-wasm-sources-as-the-provider-mechanism.md). Every one of those strings is attacker-influenced and must not be trusted to form a path. A partially written archive must never be visible as a complete one.

### The deployment has one disk

A single Compose host with a `/library` volume. There is no object store, no second node, and no replication tier.

## Decision

Package each chapter as one CBZ file containing an embedded `ComicInfo.xml`, and write it to a local volume behind the `LibraryStore` port.

### Format and tooling

- **`zip = "8.6"`.** Use the stable line. Version 9.0.0 is in prerelease (`9.0.0-pre3`) and the library writer is not a place to run a prerelease.
- Store page images **without compression** (`Stored`, not `Deflate`). JPEG and WebP page images do not compress further; deflating them costs CPU and produces a larger-than-necessary write for no gain. Compress only `ComicInfo.xml`.
- **Target ComicInfo schema v2.0.** Emit v2.1 draft fields only if a specific need appears, and never in place of a v2.0 field.
- Write page entries in zero-padded filename order so every reader sorts them identically, regardless of the source's own naming.
- Populate at minimum `Series`, `Number`, `Volume`, `Title`, `Summary`, `Genre`, `LanguageISO`, `PageCount`, `Web`, `Year`/`Month`/`Day`, and `Manga` — using `YesAndRightToLeft` when the source indicates right-to-left, and `Unknown` rather than guessing.

### Layout: optimize for other readers

Both Komga and Kavita expect one directory per series, with volume and chapter markers in the filename. Adopt that shape:

```
/library/
  <Series Title>/
    cover.jpg
    <Series Title> v03 c021.cbz
```

This replaces the draft's `<Title> [<source-slug>]/` directory and `<Title> - Vol.03 Ch.021.cbz` filename. Both draft forms are less likely to parse cleanly in third-party scanners, and the whole point of CBZ is that those scanners work.

Source identity does not belong in the path. It lives in the database and in the archive's `ComicInfo.xml` (`Web`, and `Notes` if needed). **Append a source slug to the directory name only to break an actual collision** between two series with the same title from different sources, and record that the suffix was applied.

> [!NOTE]
> The draft also placed a `series.json` at the series level. Keep it if it is useful to this application, but treat `ComicInfo.xml` inside each archive as the source of truth for interoperability. `series.json` is not part of the Anansi schema and is not read consistently by other tools.

### One library root

**v1 supports exactly one configurable library root.** Multiple roots are deferred.

The reason is that several decisions quietly assume one root, and each would need its own answer under multiple roots: which root a new series lands in, what happens when the chosen root fills up, whether a series can span roots, what `disk.library_free_bytes` reports, and how `/readyz` decides the library is writable. Those are real product questions, not configuration. Keep the port shaped so a second root is addable — `LibraryStore` should never return a bare path the rest of the system interprets — but do not build for it now.

### Writing safely

- **Sanitize every path component derived from source data:** strip path separators and control characters, normalize Unicode to NFC, reject `.` and `..`, reject Windows reserved names so the volume stays portable, collapse whitespace, and cap each component at 255 bytes measured in UTF-8 rather than characters.
- **Verify containment after construction.** Canonicalize the final path and assert it is under the library root. Sanitizing inputs and checking the result are different controls; keep both.
- **Write atomically.** Build the archive in a temporary directory, `fsync` the file, then `rename` into place.

> [!IMPORTANT]
> `rename` is atomic only within a single filesystem. The temporary directory must live **inside the library root**, not in `/tmp`. If it does not, the "atomic rename" silently becomes a copy across a mount boundary, and a crash mid-copy leaves a truncated archive that looks like a finished download.

- **Record the checksum** computed during the write, and use it as the `ETag` for `GET /downloads/{chapter_id}/file`.
- **Support `Range` requests** on that endpoint so a large CBZ resumes rather than restarting.

## Consequences

### What you gain

- **The library outlives the application.** Point Komga or Kavita at the same volume and it works. If this project is abandoned, the user keeps a readable, standard library — which is a real product property, not a side effect.
- **No storage service to run.** No MinIO container, no credentials, no bucket policy, no lifecycle configuration on a single-VM deployment.
- **Streaming is a file read.** `tokio::fs` plus `Range` support, no presigned URLs and no proxying through a storage SDK.
- **Verification is cheap.** Stored checksums make "is this download intact" answerable without re-fetching, and make the `ETag` free.
- **Debugging is direct.** A failed package is a file you can `unzip -l`.

### What it costs you

- **Durability is entirely the operator's problem.** One disk, no replication, no versioning. The `pg_dump` sidecar protects metadata but not the library, so a disk failure loses downloaded content while the database still references it. That asymmetry needs to be written down and handled, not assumed.
- **Filesystem portability is a live concern.** Case sensitivity differs between ext4, APFS, and NTFS, and Unicode normalization differs too, so two titles that are distinct on Linux can collide elsewhere. NFC normalization and the reserved-name rejection above are what keep a volume movable.
- **Path handling is security-relevant code.** Zip-slip and traversal are the classic failure here, and the inputs come from third-party sources. This is the part of `crates/packaging` that deserves fuzzing rather than only unit tests.
- **`Stored` archives are larger than the theoretical minimum.** That is the intended trade: page images are already compressed, and the CPU is better spent elsewhere.
- **Disk exhaustion is a first-class failure mode.** A download that fills the volume must fail cleanly, leave no partial file, and surface as a distinct error rather than a generic packaging failure.
- **One root is a real limitation** for anyone whose library outgrows a single disk, and lifting it is a design change rather than a setting.

### Follow-up work this decision creates

1. **Fuzz the path builder.** Feed it source-derived titles including traversal sequences, null bytes, overlong UTF-8, right-to-left overrides, and 300-character names, and assert every result stays under the root. `cargo-fuzz`, proportionate to the fact that these strings come from untrusted extensions.
2. **Put the temp directory under the library root** and add a test that asserts it, since this is the failure that produces corrupt archives rather than errors.
3. **Handle `ENOSPC` explicitly** in the packaging handler: delete the partial file, mark the job failed with a distinct error, and make sure `/readyz` reports the library as not writable.
4. **Validate against a real reader.** Generate a CBZ in a test and assert it opens correctly in at least one third-party scanner's parsing rules — ideally by checking the archive against the Anansi v2.0 schema and the filename conventions Komga and Kavita document.
5. **Decide cancellation semantics**, which the architecture leaves open: whether cancelling a running download removes partial files or keeps them for resume. With atomic rename, nothing partial is ever visible in the library, so the only question is whether the temp directory is preserved. Recommend deleting it on cancel and treating resume as a future feature, since resumable state is a second thing to keep consistent.
6. **Write the library runbook:** what to back up (the volume, separately from `pg_dump`), how to restore, how to recover when the database references a file that no longer exists, and how to reconcile after a manual move of the volume.
7. **Add a reconciliation job** that detects `downloaded_chapter` rows whose files are missing and marks them so, so the UI does not offer a read that will fail.

## Alternatives considered

| Alternative | Interoperable with other readers | Extra infrastructure | Outcome |
|---|---|---|---|
| CBZ on a local volume | Yes, universally | None | **Chosen** |
| Object storage (S3 or MinIO) | No | A service and credentials | Rejected. Defeats the main product benefit. |
| Loose image files per chapter | Partially | None | Rejected. Weaker portability, more inodes. |
| CB7 or CBT archives | Poorly | None | Rejected. Worse reader support for no gain. |
| Images as database blobs | No | None | Rejected. Wrong tool. |

### Object storage

S3, or MinIO for a self-hosted equivalent, offers durability, versioning, lifecycle policies, and growth past one disk. On a multi-host deployment it would be the right answer.

Rejected because it removes the property the format was chosen for. Komga, Kavita, and every offline reader open files from a filesystem; they do not speak S3. Storing the library in a bucket makes it readable only through this application's API, which is the outcome CBZ exists to avoid. It also adds a container, credentials, and a lifecycle configuration to a single-VM deployment, and turns chapter streaming into either presigned-URL generation or proxying through an SDK.

The `LibraryStore` port keeps this reversible. If the deployment ever becomes multi-host, an S3 adapter is an implementation of an existing trait — though the interoperability loss would still need a separate answer, such as an export path.

### Loose image files in per-chapter directories

Writing `001.jpg`, `002.jpg` into a chapter directory avoids the ZIP writer entirely, makes partial downloads resumable by construction, and is readable by some scanners.

Rejected on portability and handling. A chapter stops being one movable, checksummable, single-`ETag` object; the file count grows by roughly the page count per chapter; and `GET /downloads/{id}/file` would have to build an archive on the fly for every request. The resumability advantage is real but modest, since chapters are small enough that restarting a failed download is cheap.

### CB7 and CBT

Better compression (7z) or a different container (tar) for materially worse reader support. Rejected: compression is not the constraint, since the payload is already-compressed images.

### Images in the database

Rejected without extended analysis. It would inflate the database, complicate backups, make streaming worse, and eliminate interoperability, in exchange for transactional consistency between metadata and bytes that atomic rename plus a reconciliation job already approximates well enough.

## Revisit this decision when

- **The library outgrows one disk.** That is the trigger for multiple roots, and the questions listed under [One library root](#one-library-root) have to be answered before the code changes.
- **The deployment becomes multi-host**, at which point a shared filesystem or an object-storage adapter behind `LibraryStore` becomes necessary, and the interoperability loss needs its own mitigation.
- **Offsite or versioned durability becomes a requirement** rather than an operator responsibility documented in the runbook.
- **ComicInfo v2.1 leaves draft status**, or a target reader requires a v2.1-only field.
- **`zip` 9.0 reaches stable** and offers something the writer needs.

## References

- [The Anansi Project: ComicInfo introduction](https://anansi-project.github.io/docs/comicinfo/intro)
- [ComicInfo schema v2.0 (released)](https://anansi-project.github.io/docs/comicinfo/schemas/v2.0)
- [ComicInfo schema v2.1 (draft)](https://anansi-project.github.io/docs/comicinfo/schemas/v2.1)
- [Komga: library and file organization](https://komga.org/docs/guides/libraries/)
- [Kavita: naming conventions and file structure](https://wiki.kavitareader.com/guides/scanner/managefiles/)
- [Kavita: ComicInfo metadata](https://wiki.kavitareader.com/guides/metadata/comics/)
- [`zip` crate](https://crates.io/crates/zip) — 8.6.0 stable, 9.0.0 in prerelease
- [`quick-xml`](https://crates.io/crates/quick-xml) — for writing `ComicInfo.xml`
- [CBZ RFC](https://github.com/hyugogirubato/cbz/blob/main/docs/RFC-CBZ.md) — directional; a community consolidation of CBZ practice, not a standards-body document
