# Human-readable TOML index manifest

Date: 2026-11-14  
Task identifier: `toml-index-manifest`  
Status: implementation plan for discussion; creating this document does not start implementation.  
Inspected baseline: `a0a0691897c6ff24985fc4db9ae51f5d77a89899` (`master`, explicit index persistence).

## 1. Outcome and agreed direction

Replace `manifest.bin` with a small, readable `manifest.toml`.

- One integer program compatibility version governs the entire saved index.
- A version mismatch invalidates the saved index completely. Rebuild rather than migrate or interpret older formats.
- Each new generation writes independent, complete segment files. Remove hard-link reuse and cross-generation file references.
- The manifest names a segments directory, rather than enumerating segments.
- An optional catalog field names the catalog file.
- Repository-local storage uses `<repo>/.objekte/.indexes/<configuration-id>/`.
- Every stored file/directory path is relative to the directory containing the manifest.
- The manifest has no self-checksum. Whole-file length/SHA-256 verification is optional and represented by a path-keyed dictionary.
- Segment identity, dimensions and active-record masks belong inside the segment files.
- Target at most 2% slower index loading; 3% is the acceptance ceiling, measured as specified below.

The current production contract has separate memory build, read-only explicit load, and explicit save operations. This work uses that contract. A program-version mismatch is not permission for the low-level read-only loader to delete files or silently build/save an index.

### Simplification is an acceptance requirement

Success requires a simpler persistence implementation as well as a readable file:

- One manifest codec and one current saved-index protocol.
- One compatibility counter maintained by developers.
- One independent-generation write path, with no reuse-versus-rewrite decision tree.
- One authoritative segment descriptor inside each segment.
- One load path: parse TOML, enumerate, map, read descriptors, restore.

Remove the superseded implementation and its state. Do not retain binary-manifest fallback, format migration, independently versioned compatibility wrappers, validation-stamp rewrite machinery, a generated binary manifest cache, or a second segment inventory file.

Keep the implementation direct: a small typed manifest, ordinary path handling, existing binary segment readers and existing writer coordination. Avoid introducing a generic storage framework, pluggable manifest backends, or a new caching subsystem for this change.

The completion report must identify the old production paths, compatibility counters and bookkeeping structures actually removed. Tests and documentation may grow; the review criterion is fewer production concepts and branches, rather than an arbitrary total line-count target.

### Repository storage directory

Rename the repository target's managed directory component from `indexes` to `.indexes`:

```text
<repo>/.objekte/.indexes/<configuration-id>/manifest.toml
```

This is a component rename beneath `.objekte`. Apply it consistently to the core resolver, source-discovery exclusions, CLI/TUI/desktop path displays, tests, examples and current documentation. Temporary and custom targets retain their own established location rules.

Repository-target loads and saves resolve the new `.objekte/.indexes` location directly. Do not add a fallback loader or migration from `.objekte/indexes`. Exclude both managed locations from source discovery when applicable, so obsolete index artifacts cannot become indexed source records. Treat retirement of recognized old owned artifacts as cleanup, never as an additional supported storage format.

No task execution may broadly delete `.objekte` or an unrecognized directory while performing this rename.

## 2. Proposed manifest

Illustrative values:

```toml
version = 1
generation = 11

repository = "5ded68d1931dd8c214ca06c19efdca91c42259465347178d902018194b2cd87d"
configuration = "a456de94c3da4b474176f0283e4e478bf48c399a3633fbdc78069e1ae0c7bcfb"

segments = "generations/000000000000000b/segments"
catalog = "generations/000000000000000b/catalog.bin"
```

`repository` and `configuration` identify the selected repository and logical indexing settings. They are identity fingerprints, not checksums of the saved files. Keeping them prevents loading an index for different source selections or schemas. Do not add an absolute root path to this manifest.

### Field semantics

| Field | Meaning |
| --- | --- |
| `version` | The single supported program/index compatibility counter |
| `generation` | Destination-local saved-state counter, separate from program version and in-memory publication revision |
| `repository` | Existing repository identity encoded as hexadecimal |
| `configuration` | Existing logical configuration identity encoded as hexadecimal |
| `segments` | Directory whose immediate `.idx` children form this generation's segment set |
| `catalog` | Optional catalog path; omission means no catalog |
| `verify` | Optional dictionary of explicitly requested whole-file verification records |

No magic string, string-length prefixes, catalog-present marker, required file lengths, segment-entry array, active masks, or manifest digest.

Use ordinary TOML integers for counters with checked conversions and overflow handling; do not reinterpret negative or out-of-range values.

### Optional verification

For example, an optional entry for a catalog:

```toml
[verify."generations/000000000000000b/catalog.bin"]
length = 29396691
sha256 = "f6b94663b2252d32bc465e43c0d6a11d71ee94ffbe0332e5bf5b01393616d24f"
```

The same dictionary can name a segment or another regular file within the managed store. Each listed entry contains an expected byte length and a SHA-256 of the entire file. Dictionary keys follow the same manifest-relative path rule.

- The default writer omits `verify`.
- An omitted entry requires neither a stored length nor a whole-file hash.
- A present entry requests verification before successful load. A missing file, length mismatch or digest mismatch is an error.
- The manifest cannot list itself for verification.
- Optional verification does not select segments: directory contents remain authoritative.
- Whole-file verification reads the selected file's bytes, even for an mmap-backed segment. Measure this opt-in cost separately from default loading.

### Relative paths

Resolve all resource paths against `manifest_path.parent()`, never the working directory or source-data root. The writer emits normalized paths inside its completed generation. Retain the managed-store rules against absolute paths, escaping the owned namespace, and following symlinks.

Copying a complete managed store to another storage base must preserve these relative references. Repository identity still identifies the original source repository; moving the source repository itself can invalidate that identity.

### Optional catalog versus a usable index

The manifest/container can represent an absent catalog. Today's full source-free `Index::try_load` needs catalog metadata, so ordinary saves include it. A catalog-less container must produce a clear missing-required-catalog error when used for full index restoration; it must not silently expose a partial index or trigger source rebuilding inside load.

## 3. Segment discovery and metadata

### No manifest entry is needed to decide what to mmap

The current loader already opens and maps every segment before installing the index. It does not use the manifest as a per-query shard-selection table.

The replacement load path is:

1. Read and deserialize the small TOML manifest.
2. Check its version and repository/configuration identity.
3. Enumerate the immediate `.idx` files in `segments`.
4. Open and mmap each file once.
5. Read a small binary bootstrap header from each mapping.
6. Group/install segments using the header's collection and ordinal.
7. Complete existing catalog, record and dictionary validation before exposing the index.

Do not read whole segment files into intermediate buffers, map files twice, or add an auxiliary binary copy of the manifest. Mapping a file does not itself read every payload page, although current validation does touch some sections.

Directory order is not meaningful. Order the small segment-descriptor collection deterministically by collection/ordinal, and reject duplicate identities. Do not infer identity from a filename.

### Bootstrap header and active mask

Extend/refactor the segment envelope so the following can be read from the beginning of the mapped file without decoding posting dictionaries:

| Metadata | Purpose |
| --- | --- |
| File type marker and unified program version | Recognize this format and reject incompatible bytes |
| Store generation | Detect a segment copied from a different saved generation |
| Collection ID and segment ordinal | Route the segment to its collection and identify it |
| Slot count | Number of record positions in this segment |
| Active count and active-domain descriptor | Restore liveness without consulting the manifest |
| Writer-validation flag | Retain the existing validated-writer fast path under the unified version |
| Section-directory location/count | Locate the existing binary index sections |

Place the active domain in a leading, independently addressed segment metadata region:

- All active: compact marker, no serialized bitmap.
- Otherwise: portable Roaring bitmap containing active slot numbers.
- Check bitmap framing, cardinality and slot bounds.

The active mask now travels with the segment. There is no `.active` sidecar. Fresh independent files for every generation make this possible without modifying another generation's data.

Consolidate duplicated collection/ordinal/slot-count fields where practical; establish one authoritative bootstrap descriptor rather than adding another independent metadata table.

The writer-validation flag records a successful validation/construction outcome. A matching program version alone is not proof of validation. A flag never proves an unvisited posting payload. Produce this evidence during save, not by writing to a store during load.

Generation and saved active-domain metadata are supplied when writing a disk snapshot. Memory-only construction does not acquire a fictitious store generation or freeze its mutable publication liveness. Existing owned/constructed section buffers can feed the snapshot writer.

Before changing the codec, specify the header field widths, offsets and mask framing in the maintained format document. Use explicit little-endian scalars and checked offsets; no Rust struct layout is persisted. Keep the descriptor fixed-size, with the variable bitmap addressed separately, so opening it is constant work per segment apart from bitmap decoding.

### Completeness without a segment list

Ordinary catalogs should record one aggregate expected segment count. Compare it with discovered files, alongside generation and identity checks, so deleting a segment cannot silently turn a saved index into an incomplete result set. This is a single count in the binary catalog, not a replacement filename list or routing table.

An empty generation has an existing empty segments directory and an expected count of zero. A missing directory is not an empty index. Apply existing segment-count limits before allocating the descriptor collection.

## 4. File size and checksums

File-backed `memmap2::MmapOptions::map` defaults to the actual file length. The loader can obtain size from the opened file and mapping; mandatory copies of file lengths in TOML are unnecessary.

Continue to check:

- Supported regular-file status and existing maximum file sizes.
- Header and section offsets/lengths against the actual mapping.
- Checked arithmetic, nonoverlap, record domains and cross-references.
- Immutability of files while readers hold mappings.

The filesystem supplies a size; it does not validate the internal index structure.

Remove the manifest's trailing SHA and mandatory catalog whole-file/body SHA checks, replacing whole-catalog verification with the optional `verify` entry. Catalog internal regions still have explicit framing and bounds.

Existing segment section/posting checks belong to the binary readers and continue to provide their current integrity checks in this first implementation. Optional whole-file verification is additional and must not become mandatory through an implicit default.

## 5. Unified compatibility and publication

### Versioning

Introduce one `PROGRAM_VERSION: u32` for saved-index compatibility. Use it for the manifest, segment/catalog envelopes, persisted subformat interpretation and writer-validation rules.

Audit and consolidate the current independently maintained manifest, segment, model, catalog, record-section, metadata and validator version counters. Repeated header values may reference the same constant; there must not be multiple compatibility counters to coordinate.

A major incompatible program revision increments this number. A patch release that preserves compatibility need not. This is an explicit counter, not the current Cargo package string `0.1.0`.

Configuration and source-adapter identities still distinguish actual user settings and adapters. Audit algorithm-version contributions to configuration hashing so internal saved-index compatibility is governed by the one program counter.

Recognized binary manifests and mismatched program versions are incompatible, never migrated. The high-level caller can rebuild through the normal memory-build flow; an explicit save can replace obsolete owned artifacts. Read-only load remains read-only. Cleanup must work even when a version change produces a different configuration namespace.

### Fresh independent generations

For every actual new saved generation:

1. Pin one complete in-memory publication.
2. Create a private staging generation directory.
3. Write a fresh catalog and every segment, including each segment's current active mask.
4. Translate current/historical catalog references into this generation's catalog.
5. Validate the produced metadata and complete file writes.
6. Synchronize and rename the completed generation directory.
7. Write a temporary TOML manifest, synchronize it and atomically replace `manifest.toml`.
8. Retire older generations through reader-safe managed cleanup.

Remove `SegmentInput::Existing`, hard-link publication, the durable-file reuse decision tree and reporting of reused/link-created files. No symlinks, hard links, or references into another generation.

Already encoded sections may be streamed into newly created files, with updated metadata, instead of reconstructing postings or rereading source data. This still produces fully independent files; it is not cross-generation file reuse.

Historical metadata needed by inactive records still has to be represented correctly. Generate the necessary historical references for the new snapshot rather than retaining append-only history merely to keep old hard-linked files meaningful.

Keep memory bounded by processing one segment at a time. A loaded index saved to another target must remain loadable there after removing the source store.

In particular, do not materialize all loaded segments and then retain them in queued writer closures. Acquire the source metadata from the pinned publication/catalog, materialize only what one output segment needs, write it, and release its temporary state before proceeding. Catalog-reference translation must finish before committing the completed catalog and manifest.

An unchanged explicit save can retain its existing no-op result: it creates no generation. If a new generation is created, all its files are new.

Preserve destination generation conflict detection, publication revision reporting, and pinned-reader behavior. Quantify the extra save time and retained disk bytes caused by full generations.

## 6. Implementation sequence

1. **Record the execution baseline.** Re-read `docs/ai/README.md`, establish the actual current commit, and preserve user files. The inspected checkout has the user's untracked Musenalm index; benchmarks must use separate disposable stores. Later implementation belongs in `integration/toml-index-manifest` and its sibling integration worktree.
2. **Establish comparable load measurements.** Reuse the existing benchmark examples, adding a small uninstrumented explicit-load mode if needed before changing the format. Run the same harness against baseline and candidate.
3. **Add the TOML codec.** Use a typed Serde structure and a normal TOML parser/pretty serializer. The lockfile already includes `toml`, but core needs a direct dependency; include that dependency addition in the implementation scope approved with this plan. Use the existing locked compatible release with the required Serde/parse/serialization features, rather than writing a parser.
4. **Implement self-describing segments.** Consolidate compatibility constants, encode the bootstrap metadata and active bitmap, add the catalog's aggregate completeness count, and update direct readers.
5. **Implement directory-based loading.** Resolve manifest-relative paths, enumerate/map once, derive segment descriptors from headers, handle optional catalog/verification, and update typed diagnostics.
6. **Implement independent publication.** Write every segment anew, simplify catalog-reference translation and persistence bookkeeping, atomically publish TOML, and remove binary-manifest migration/reuse paths.
7. **Integrate callers and documentation.** Update file-name assumptions, load/save reports, obsolete-format handling, examples and current format documents. Document the short TOML schema and the single-version rebuild policy.
8. **Validate and measure.** Complete the checks below, report results, and follow the repository completion/commit/push workflow only after execution is separately authorized.

Primary code: `managed_store.rs`, `segment_format.rs`, `hybrid_segment_codec.rs`, `segment_metadata.rs`, `generation_catalog.rs`, `lifecycle_storage.rs`, `index_storage.rs`, `configuration.rs`, and `lib.rs` under `crates/core/src/`.

### Required legacy-code removal

Removal is part of implementation, not a follow-up task:

1. **Independent compatibility counters:** replace and delete `MANIFEST_VERSION`, segment `FORMAT_VERSION`, `MODEL_VERSION`, `VALIDATOR_VERSION`, `HYBRID_VALIDATOR_VERSION`, `META_VERSION`, `RECORD_VERSION`, and the catalog/metadata-specific saved-format `VERSION` constants. Update call sites to the shared `PROGRAM_VERSION`; do not retain the old names as aliases or deprecated wrappers. Remove old model-version-specific hashing helpers once their callers use the unified counter.
2. **Older-format support:** delete the superseded binary manifest encoder/decoder, alternate-format dispatch, migration functions, old-format fixture encoders, and validation-stamp rewrite code. Remove compatibility implementations hidden behind `cfg(test)` or unused feature flags as well. Tests for rejecting old data can use small unsupported-format fixtures; they do not need a maintained old reader/writer.
3. **Discontinued SHA writing:** delete unconditional manifest digest construction/appending and catalog whole-file/body digest construction. Remove their obsolete stored fields, fixed digest/trailer sizes, checksum-only buffers and bookkeeping.
4. **Discontinued SHA verification:** delete mandatory manifest/catalog hash passes, comparisons and associated old error branches from the open path. The optional `verify` dictionary is the sole whole-file verification path for those resources. Absence of an entry means no such hash computation, including hidden preliminary or duplicate checks.
5. **Obsolete supporting code:** remove unused imports, helpers, metrics, report fields, assertions, checksum-repair fixture utilities and tests whose only purpose was the discontinued protocols. Rewrite substantive validation tests against the current format.
6. **Audit remaining hashes and versions:** classify each retained use by its current purpose. Repository/configuration identity, source fingerprints, optional whole-file verification and the section/posting checks explicitly retained in section 4 are distinct from the deleted manifest/catalog hashes. Source adapters' real identities are distinct from obsolete internal format counters. Do not indiscriminately remove `sha2` or adapter identity handling.

Only minimal old-artifact recognition needed for an incompatibility result or ownership-checked cleanup may remain. It must not decode or migrate an old index for use.

Before completion, search production code and tests for the deleted symbols and paths, inspect all remaining manifest/catalog hash call sites, and list any deliberately retained legacy-name references with their purpose. Compile all targets after this removal. The report must demonstrate that the old behavior was deleted rather than disabled or left as a second implementation.

### Concrete change map

Paths below are relative to `crates/core/` unless qualified otherwise. Symbols refer to the inspected baseline; recheck them against the execution revision.

| Location | Required change |
| --- | --- |
| `Cargo.toml`, repository `Cargo.lock` | Declare the TOML codec dependency and its needed features. |
| `src/lib.rs`, `src/configuration.rs`, `src/index_storage.rs` | Define the shared program counter, delete independent saved-format/model/validator gates and exports, and keep repository/configuration identity resolution consistent across memory build, save and load. Change both repository owned-namespace and final destination resolution to `.objekte/.indexes`, with matching discovery exclusions. |
| `src/managed_store.rs`: `Manifest`, `CatalogRecord`, `SegmentRecord`, `encode_manifest`, `decode_manifest*` | Replace the binary manifest model/codec with the TOML schema. Remove manifest-only cursor/framing helpers, per-segment manifest records, mandatory length/hash records and the trailing digest. |
| `src/managed_store.rs`: `open_generation`, `read_catalog_typed` | Resolve declared relative paths, discover segment files, map once, obtain descriptors from segments, and run the optional verification dictionary only when present. |
| `src/managed_store.rs`: `OpenedGeneration` | Replace parallel filename/length/identity/active/trust vectors with one in-memory descriptor per opened segment. Derive lengths from actual files/mappings rather than maintaining another persisted length table. |
| `src/managed_store.rs`: publication, `SegmentInput`, migration/stamp helpers, cleanup | Remove `Existing`/hard-link handling and legacy migrations. Write independent files and atomically publish `manifest.toml`. Update cleanup/file recognition for the new layout without interpreting old indexes as loadable. Remove post-load stamp rewrites. |
| `src/segment_format.rs` | Encode/decode the small bootstrap descriptor, active-domain location and current program version; expose them through the existing mapped reader. Maintain checked section addressing. |
| `src/hybrid_segment_codec.rs`, `src/segment_metadata.rs` | Supply identity, dimensions, liveness and validation evidence to the new envelope. Consolidate duplicated descriptor fields and independent subformat versions. Keep persisted metadata source-free to rewrite. |
| `src/generation_catalog.rs` | Use the shared compatibility counter, remove mandatory body hashing, record expected segment count, and encode the complete current generation's required file/history metadata with correct translated references. |
| `src/lifecycle_storage.rs` | Replace reuse selection in `flush_generation` with bounded independent writes. Remove `DurableSegment`/`durable_segments` and `current_refs` bookkeeping used to decide file reuse. Obtain source metadata through the pinned publication and its catalog. Reduce destination state to save/conflict/no-op tracking actually required. Restore from the discovered descriptors. |
| `src/lifecycle.rs` and load/save report consumers | Adapt opened-generation installation, destination bookkeeping and reports to the simplified structures. Remove obsolete reused-file and independently versioned stamp fields instead of leaving permanently zero compatibility fields. |
| Core storage/codec/lifecycle tests; CLI and desktop persistence tests | Replace binary-offset/migration/reuse expectations with tests of the new contract. Preserve substantive corruption, liveness, concurrency and read-only-load coverage. |
| `crates/cli/src/loader.test.rs`, `crates/cli/tests/obj_process.rs`, desktop backend persistence tests, and storage path displays | Change repository-path expectations from `.objekte/indexes` to `.objekte/.indexes`; verify that explicit saves create the new location and ordinary memory builds do not create it. |
| `crates/core/examples/` and benchmark result tooling | Distinguish explicit load from build/save timers, adapt obsolete metrics, and run the same uninstrumented load harness on both revisions. |
| `docs/development/` persistence format documents | Publish the TOML schema, precise binary bootstrap layout, version policy, independent-write behavior and optional-verification semantics as one consistent current contract. |

### Small contract details to settle in the first implementation

- Emit deterministic, pretty TOML with stable field order. Accept normal TOML whitespace, comments and key order. Reject misspelled/unknown schema fields with useful parse diagnostics rather than silently ignoring a user's edit.
- Generated manifest paths must fit the TOML string representation without lossy conversion. Generated resource names are controlled ASCII; do not serialize arbitrary source-file paths into TOML.
- Within the selected namespace, a current `manifest.toml` is authoritative if obsolete `manifest.bin` also exists. If only the old binary manifest exists there, report incompatibility, not an empty store. A malformed TOML manifest does not fall back to an older snapshot. This does not authorize searching the discontinued repository directory.
- Enumerate immediate regular `.idx` children only. Unrelated files are not segments; a malformed `.idx` is an error, not silently skipped. Do not recursively discover nested segment directories.
- Verification entries are checked against the same opened file objects used for loading when applicable. Duplicate path aliases must not cause redundant hashing or contradictory expectations.
- Preserve source metadata ownership separately from destination persistence history: removing reuse bookkeeping must not make load-then-save depend on reopening an older store.

Implement the working read/write path first, exercise it end to end, then measure and optimize only the measured loading costs. A new compatibility layer or binary mirror is not an acceptable shortcut for the performance gate.

## 7. Verification and performance acceptance

### Functional checks

Exercise meaningful save/load behavior, including:

- Human-edited whitespace/comments, readable round-trip output, absent/present catalog and absent/present verification dictionary.
- Relative paths resolved from a different working directory and after moving the whole managed store.
- Repository-target save/load at `.objekte/.indexes`, no load fallback to `.objekte/indexes`, and source discovery excluding both applicable managed locations.
- Missing/mismatched versions and recognition of the obsolete binary format.
- Empty and multi-collection generations, directory order changes, duplicate identities, missing segments and mixed-generation files.
- All-active, partially retired and fully retired segment domains; deleting/updating source records must not resurrect them after save/load.
- Actual file length differing from optional verification metadata, hash mismatch, truncated headers and out-of-bounds sections.
- Two successive generations containing independent files, zero link reuse, and an older pinned reader remaining usable after a new save.
- Interrupted publication leaving the old manifest usable; concurrent writer conflicts.
- Read-only source-free load, load A/save B/remove A/load B, and save without changing the active index's backing.

Run the workspace checks appropriate to this cross-component persisted-format change:

```sh
cargo test --workspace --locked -- --quiet
cargo check --workspace --all-targets --locked
```

Use the existing Musenalm query campaign to compare result digests for all 220 ASCII and 220 Unicode cases on saved/reopened indexes, including representative first queries after load.

### Define the 2–3% budget precisely

The budget applies to **end-to-end successful explicit index loading until query-ready**, with default optional verification omitted. It includes TOML parsing, directory enumeration, file opening/mapping, metadata restoration and required validation.

For every agreed load scenario:

```text
ratio = candidate median open time / baseline median open time
target: ratio <= 1.02
hard ceiling: ratio <= 1.03
```

Apply this separately to ASCII and Unicode and to ordinary and fragmented/many-segment Musenalm stores. Do not average a failing case away with a faster one. Also report p95, absolute milliseconds and run-to-run variation; a reproducible p95 regression above 3% requires investigation rather than declaring an unqualified pass.

The existing approximately 253 ms ASCII result suggests a 3% allowance of about 7.6 ms, but that historical result is context only. Establish a new paired baseline on the execution machine.

### Measurement method

- Ordinary release builds, identical toolchain/features, machine, fixture, indexing options and query-cache settings.
- Independent baseline/candidate stores with identical logical contents and comparable segment counts/distribution. Old/new formats cannot use one byte-identical store.
- At least 30 alternating fresh-process pairs per warm-cache load scenario after warm-up. Preserve raw samples and exact commands.
- Repeat using the existing page-eviction method for cold-cache scenarios, with enough paired trials to distinguish a 3% change. Report these as page-evicted measurements, not cold boot.
- If uncertainty overlaps the acceptance ceiling, collect further trials; do not claim that a noisy difference proves compliance.
- Time default opens without allocation counters or detailed phase tracing. Run allocation and phase diagnostics separately.
- Measure time to first representative query and verify zero source reads/parses during restore. Moving restoration work into the first query is not an acceptable optimization.
- Profile TOML parsing, directory enumeration/header access, catalog work and segment installation separately if the load ceiling fails. Do not weaken correctness validation simply to meet the budget.

`bench_managed_open_alloc` currently times `Index::try_load` with an instrumented allocator. Use it for allocations; add an uninstrumented mode to that example or another existing runner for acceptance timing. `bench_musenalm` currently labels some build/save timings `open_ns`; those are not explicit-load measurements and must not be used as the load gate.

Benchmark explicit initial save, changed save, unchanged save, peak memory and retained disk usage using the existing `bench_managed_flush`/lifecycle runners. Independent full-file writes are an accepted save-side tradeoff; the 3% load ceiling is not a promise of unchanged save cost.

Measure verification-enabled opens separately and state which files were hashed. Reading/hashing entire optional files is excluded from the default-load budget.

### Completion criteria

- The TOML schema above is sufficient; no mandatory per-segment manifest entries or binary mirror.
- Repository-target resolution, displays, tests and current documentation consistently use `.objekte/.indexes`.
- Segment metadata and liveness restore correctly from the segment files.
- One compatibility counter, no older-format migration, no cross-generation file reuse.
- Default load requires neither manifest nor catalog whole-file SHA verification.
- Every ordinary saved generation is complete, self-contained and source-free to load.
- The simplification requirements and concrete removals above are implemented and reviewed, including deletion of superseded production paths.
- Legacy versioning, migration codecs and discontinued mandatory manifest/catalog SHA code are removed, including obsolete test-only implementations and helpers; retained hashes have a documented current purpose.
- Functional checks and query equivalence pass.
- The measured default-load regression meets the 3% ceiling, with the 2% target reported.
- Any unresolved performance failure remains a blocker for completion; obtain a user decision before changing the design or relaxing the budget.

## Reference documentation consulted

- Current repository persistence code and `docs/ai/generated/explicit_index_persistence_results.md`.
- `bench/results/explicit-index-persistence/2026-11-14/README.md`.
- Rust `toml` documentation for typed `from_str` and `to_string_pretty`.
- Rust `memmap2` documentation: file-backed mapping length defaults to file length; backing files must remain immutable while mapped.

## Implementation report

- Task identifier: `toml-index-manifest`
- Completion date: 2026-11-14
- Base commit: `a0a0691897c6ff24985fc4db9ae51f5d77a89899` (`master`)
- Integration branch: `integration/toml-index-manifest`
- Integration worktree: `/home/simon/source/objekte-integration-toml-index-manifest`
- Implementation commit: `b31db7b2eb97bfe3d5cf9e047a823845b299d647`
- Performance and issue report commit: `f4337dc5c2`

### Summary

The saved index now uses a small readable `manifest.toml` and one `PROGRAM_VERSION` for the manifest, each segment, the catalog and all persisted subformats. Segment files are self-describing: the 88-byte bootstrap header carries generation, collection, ordinal, slot count, active mask and writer-validation flag. The catalog records one aggregate expected segment count. Loading resolves manifest-relative paths, enumerates immediate `.idx` files, maps each file once, reads its descriptor and validates the catalog before exposing the index.

Every new generation writes a fresh catalog and every segment file; hard-link reuse, cross-generation references, binary-manifest decoding, migration and post-load validation-stamp rewrites are removed. The repository target now uses `<repo>/.objekte/.indexes/<configuration-id>/`, and both `.objekte/.indexes` and the obsolete `.objekte/indexes` are excluded from source discovery. An explicit save can replace an incompatible binary or version-mismatched store while a read-only load remains read-only and reports incompatibility.

### Implemented changes

- Added the typed TOML manifest with `version`, `generation`, hexadecimal repository/configuration identities, a `segments` directory, an optional `catalog` and an optional path-keyed `[verify]` dictionary. Unknown fields, absolute/escaping paths and self-verification are rejected. Comments, whitespace and field reordering are accepted.
- Added the self-describing segment envelope with a checked all-active marker or portable Roaring active domain, collection/ordinal identity, generation and writer-validation flag. Removed the manifest segment list and `.active` sidecar.
- Added the catalog expected segment count and removed mandatory catalog body/file SHA computation and checking. Optional whole-file verification is the only manifest/catalog whole-file hash path.
- Removed binary manifest codecs, flat-v1 migration, validation-stamp upgrades, hard-link reuse, `SegmentInput::Existing`, `DurableSegment`/`current_refs` reuse bookkeeping, reused-file/link report fields and mandatory manifest/catalog digest fields.
- Consolidated `MANIFEST_VERSION`, segment `FORMAT_VERSION`, `MODEL_VERSION`, `VALIDATOR_VERSION`, `HYBRID_VALIDATOR_VERSION`, `META_VERSION`, `RECORD_VERSION`, metadata `VERSION` and catalog `VERSION` into `PROGRAM_VERSION`. Removed `configuration_hash_for_model` and the obsolete model-version hashing helper.
- Changed the repository owned namespace to `.objekte/.indexes`, updated CLI/desktop test expectations and path displays, and excluded both the current and renamed managed directories from discovery.
- Removed permanently-zero stamp/reuse status fields from core, CLI JSON and desktop UI.
- Added an uninstrumented `--no-count` mode to `bench_managed_open_alloc` and an explicit-load `--load` mode to `bench_musenalm`.
- Documented the current TOML schema, segment bootstrap layout, version policy, independent-write behavior, optional verification and `.indexes` location in `docs/development/`.
- Added `docs/ai/generated/toml_index_manifest_performance.md` with raw-command summaries, paired load results, save tradeoffs and query-equivalence results.
- Recorded a pre-existing fragmented-store mutation failure in `docs/ai/issues/bug/fragmented-loaded-save-identity-mismatch.md`.

### Removed production paths and compatibility bookkeeping

The completion review searched production code and tests for the deleted symbols and paths. No remaining production references exist for `MANIFEST_VERSION`, `FORMAT_VERSION`, `MODEL_VERSION`, `VALIDATOR_VERSION`, `HYBRID_VALIDATOR_VERSION`, `META_VERSION`, `RECORD_VERSION`, `manifest.bin` decoding, `encode_manifest`/`decode_manifest`, `SegmentInput::Existing`, `DurableSegment`, `durable_segments`, `current_refs`, `record_validation_stamps`, hard-link publication or migration helpers.

Retained hashes and version-like values and their purposes:

- Repository and configuration identity hashes use SHA-256 and `PROGRAM_VERSION` as a configuration input.
- Optional manifest `[verify]` entries are the only whole-file length/SHA-256 path for persisted resources.
- Segment section SHA-256 and posting short checksums remain the existing structural/payload integrity checks.
- Source fingerprints, source-adapter versions, `BIGRAM_VERSION`, `COLLATION_VERSION`, custom binding versions and `CONFIGURATION_HASH_VERSION` continue to describe actual source adapters or algorithm/settings identity; they are not saved-index compatibility counters.
- `manifest.bin`, `OBJHMAN1`, flat segment/catalog files and `.tmp-*` generation directories are recognized only as obsolete owned artifacts for incompatibility reporting or ownership-checked cleanup; they are never decoded into a usable index.

### Validation

```sh
cargo test --workspace --locked -- --quiet
cargo check --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling
```

All tests and checks passed. The workspace suite includes 620 core library tests, 171 desktop tests, 90 CLI library tests, CLI integration tests and doc tests. One desktop export test raced between file creation and writing during an intermediate run; it failed once, then passed alone and in the complete desktop suite on rerun. It does not involve persistence code.

Functional coverage includes: human-edited TOML comments/reordering and unknown-field rejection; readable round-trip output; absent/present catalog and verification; relative paths; missing/mismatched versions; obsolete binary manifest incompatibility; explicit replacement of incompatible stores; empty/multi-collection generations; directory order independence; duplicate identities; copied/mixed-generation segments; sparse and all-active domains; segment length/header/section corruption; two independent generations; pinned readers; interrupted publication; read-only loads; load A/save B/remove A/load B; and repository `.objekte/.indexes` save/load with no fallback to `.objekte/indexes`.

### Performance

Full commands, environment, raw sample locations and tables are in `docs/ai/generated/toml_index_manifest_performance.md`.

- Warm explicit load, 30 alternating fresh-process pairs per scenario: ASCII 8-segment ratio 0.9513, Unicode 8-segment ratio 0.9420, ASCII 71-segment fragmented ratio 0.9440. The 2% target and 3% ceiling were met.
- Page-evicted load, 5 alternating pairs per scenario: ratios 0.9154 (ASCII), 0.9806 (Unicode) and 0.9569 (ASCII fragmented). `fincore` reported zero resident index bytes after eviction. This is page eviction, not cold boot.
- Optional verification in a candidate ASCII store: three opens took 235.447–240.407 ms versus 232.250 ms default; only the catalog was hashed. Default load performs no manifest/catalog whole-file hashing.
- Initial ASCII save, three paired runs: median 322.984 ms baseline versus 300.923 ms candidate (ratio 0.932); process peak RSS median 692,864 vs 702,640 KiB (ratio 1.014). Unchanged save was 0.849 vs 0.856 ms (no-op still creates no generation).
- Changed save after updating ten files in a loaded single-generation store: baseline 236.0 ms and 2,938,848 segment bytes; candidate 597.3 ms and 112,404,521 segment bytes. Two mutation cycles on 71-segment stores wrote 8.3/10.5 MB per baseline generation versus 117.8/119.9 MB per candidate generation; retained store size after three generations was 210 MiB versus 419 MiB.
- Query equivalence: all 220 ASCII and all 220 Unicode case digests matched between baseline saved/reopened, baseline memory build, candidate saved/reopened and candidate memory build. Every explicit-load campaign reported zero source reads and zero source parses before its first query.

A fragmented-store third changed-save timing was omitted because both revisions hit the pre-existing mutation/partition re-sealing bug in `docs/ai/issues/bug/fragmented-loaded-save-identity-mismatch.md`. The first two fragmented changed saves completed on both revisions and supplied the byte and retained-size comparison. This is a mutation-path defect present at the base commit, not a load-budget or manifest failure.

### Deviations and limitations

- No writer API was added to request `[verify]` entries. The schema, reader and default omission are implemented; users or tools can add verification records explicitly. This matches the agreed default and avoids growing the save API for an opt-in check.
- Automatic cleanup does not delete sibling configuration directories belonging to a different configuration identity. A version change that produces a new configuration namespace leaves the old owned namespace untouched; read-only load never deletes files, and prepared stores exclude both managed locations from discovery.
- An incompatible store is replaced by a new explicit save starting at a new generation; obsolete generation directories are retained by the normal age/count grace policy rather than deleted during the same save.
- The fragmented repeated-mutation bug is recorded for user review and is not fixed here because it reproduces at the base commit and belongs to mutation/partition publication rather than the saved-index format.

### Worktree and cleanup

Integration worktree retained for the user at `/home/simon/source/objekte-integration-toml-index-manifest`. No subordinate worktrees were created, so no subordinate-worktree cleanup was required. Temporary benchmark helpers and baseline-worktree measurement patches were removed; the baseline worktree is clean. Disposable stores and raw benchmark samples remain under the integration worktree's ignored `target/toml-index-manifest/bench/` directory.
