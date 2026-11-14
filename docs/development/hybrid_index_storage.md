# Hybrid index storage contract

Current saved-index contract plus historical design and benchmark record. The active persistence contract uses memory construction and explicit `Index::try_load` / `Index::flush_to`; automatic synchronization and full-residency selection were removed by `explicit-index-persistence`. The readable TOML manifest, unified program version, self-describing segments and independent generation writes were introduced by `toml-index-manifest`.

Task `hybrid-index-storage`. This document fixes the wire and lifecycle contracts before replacing the current snapshot implementation. The baseline is `da8fde68472b8646a63a05b3b3654c58de877301`.

## Current saved-index contract

### Manifest

The store manifest is `manifest.toml`, replaced atomically by rename. It is a small typed TOML document:

```toml
version = 1
generation = 11
repository = "<64 lowercase hex characters>"
configuration = "<64 lowercase hex characters>"

segments = "generations/000000000000000b/segments"
catalog = "generations/000000000000000b/catalog.bin"

[verify."generations/000000000000000b/catalog.bin"]
length = 29396691
sha256 = "<64 lowercase hex characters>"
```

`version` is the single `PROGRAM_VERSION` for the manifest, segment and catalog envelopes and every persisted subformat. A mismatch invalidates the saved index; there is no migration or fallback reader. `generation` is the destination-local saved-state counter and must be nonzero and strictly increasing. `repository` and `configuration` are hexadecimal identity fingerprints, not file checksums. `segments` names the directory whose immediate regular `.idx` children form the segment set; it is not a segment list. `catalog` is optional: an ordinary save includes it, while omission means no catalog and a full restore reports a missing-required-catalog error rather than exposing a partial index. `verify` is optional; each listed path is resolved relative to the manifest's directory and is checked for exact byte length and whole-file SHA-256 before the index is exposed. The default writer omits it. The manifest cannot verify itself.

All resource paths are normalized relative paths resolved against the manifest's parent directory, never the working directory or source root. Absolute paths, parent-directory components and symlinked managed directories are rejected. Unknown TOML fields are rejected with a parse diagnostic. The manifest has no self-checksum and no required per-file lengths or hashes. A current `manifest.toml` is authoritative when an obsolete `manifest.bin` also exists; a lone binary manifest reports incompatibility. Recognized old owned artifacts are removed only as cleanup, never decoded for use.

### Segment envelope and self-description

Each persisted segment begins with an 88-byte explicit little-endian header, followed by a 56-byte-per-entry section directory, then the optional active-domain bitmap and the section payloads:

| Offset | Width | Meaning |
| ---: | ---: | --- |
| 0 | 8 | ASCII file-type marker `OBJHSEG1` |
| 8 | 4 | `PROGRAM_VERSION` |
| 12 | 4 | header length, 88 |
| 16 | 8 | store generation |
| 24 | 4 | collection ID |
| 28 | 4 | flags: bit 0 writer-validated, bit 1 all-active |
| 32 | 8 | segment ordinal, nonzero |
| 40 | 4 | slot count |
| 44 | 4 | active count |
| 48 | 8 | active-domain byte offset, immediately after the directory |
| 56 | 4 | active-domain byte length, zero for the all-active marker |
| 60 | 4 | section count |
| 64 | 8 | section-directory offset, 88 |
| 72 | 8 | section-directory byte length |
| 80 | 8 | reserved zero |

The active domain is a compact all-active marker or a portable Roaring bitmap of active slot numbers. Its framing, cardinality and slot bounds are checked before the segment is exposed. No `.active` sidecar exists. Section entries are `(kind u32, flags u32, offset u64, length u64, SHA-256 [32])`; kinds are strictly increasing, spans do not overlap, and every offset is checked against the mapping. Required non-posting sections are checked before installation; posting payloads retain their existing per-payload checks and first-use validation. The writer-validation flag means the segment was produced through the validated construction path under the current program version; it never proves an unvisited posting payload.

The loader reads the manifest, enumerates immediate `.idx` files, orders descriptors deterministically by collection/ordinal, maps each file once, reads its header, and completes catalog, record and dictionary validation before exposing the index. Directory order is not identity. The generation catalog records one aggregate expected segment count; a missing segment therefore fails the open instead of silently returning incomplete results.

### Catalog envelope

The catalog starts with a 28-byte header: `OBJHCAT1` (8 bytes), `PROGRAM_VERSION` (`u32`), body length (`u64`), expected segment count (`u32`), and reserved zero (`u32`). There is no mandatory body or file SHA-256; catalog internal regions keep explicit framing, bounds and cross-reference checks. The 104-byte body header and all tables described below are unchanged apart from the unified version and the expected segment count.

### Publication and cleanup

Every actual new saved generation is independent: the writer pins one publication, creates a private staging generation directory, writes a fresh catalog and every segment (including its current active mask), translates catalog references into the new generation's catalog, validates the produced files, syncs, renames the generation directory, writes a temporary TOML manifest, syncs it, and atomically replaces `manifest.toml`. No hard links, symlinks or references point into another generation. Already encoded sections may be streamed into new files with updated metadata, and only one output segment is materialized at a time. An unchanged explicit save is a no-op and creates no generation. Generation conflict detection, pinned readers and positive ownership checks remain mandatory.

Garbage collection runs under the writer lock after publication. It retains the current generation and seven predecessors (`RETAINED_GENERATIONS = 8`), waits at least 120 seconds (`MIN_RETENTION_AGE`), examines at most 64 entries per commit, and removes only strict generation names, owned directories and known owned files. Unknown entries, symlinks and foreign-owned files are preserved with a reclaim diagnostic. Pinned readers keep their mapped immutable bytes usable after directory removal. Root-level recognized temporary manifests and obsolete flat-format owned artifacts may be removed; unknown files are not.

### Version policy

A major incompatible program revision increments `PROGRAM_VERSION`. Patch releases that preserve saved-index compatibility need not. The configuration identity still covers actual source adapters, schema, indexing and collation settings, and uses the same program counter; it is not a second compatibility counter. The saved-index loader performs no migration, stamp rewriting, binary-manifest fallback or cross-generation reuse.

## Policy and identity

`IndexStoragePolicy` is the sole core decision about index location and residency. Its default is `MemoryOnly` plus `FullyResident`. Memory-only opening does not inspect, create, synchronize or clean an index store. Disk modes default to `OnDemand`; `FullyResident` changes only ownership of queryable bytes and keeps synchronization enabled. `MemoryOnly` with `OnDemand` is an error.

Configured repository convenience opening now defaults to an early source-backed memory build. It reads `.objekte/collections` but does not probe or select the legacy snapshot path. Explicit legacy cache options remain temporarily for existing tests and callers while the new disk path is integrated, and will be removed before this task is complete.

The selected data root is canonicalized. The repository identity is SHA-256 over the root's exact platform path bytes plus length-prefixed, normalized collection names and relative source selections. The configuration identity uses the existing checked configuration input set: source/format/extractor identity and version, schema and indexed fields, inference settings, alphabet, folding, collation, and sorting. It excludes modification times and file sizes. The TOML manifest stores the two 32-byte identities as hexadecimal, so a path-hash collision cannot silently bind another repository. Changing a source file reconciles inside the same namespace.

Managed paths are:

| Choice | Directory |
| --- | --- |
| System temporary | `<platform-temp>/objekte-<user-id>/indexes/<repository-id>/<configuration-id>/` on Unix; the platform's private temp location on other systems |
| Repository | `<data-root>/.objekte/.indexes/<configuration-id>/` |
| Custom | `<selected-base>/objekte-indexes/<repository-id>/<configuration-id>/` |

The system temporary owner directory must be private (`0700` on Unix), checked for symlinks and current ownership before writes. Every existing directory beneath the selected owned namespace must belong to the current user and not be writable by other users; unsafe layouts are rejected before generation children are created. Lock, manifest and segment files must be owned regular files opened without following symlinks. The selected custom base must exist. `IndexStoragePolicy::prepare` returns one `PreparedIndexStorage` containing both effective collection definitions and their resolved identity/path. Its source-definition step appends the selected owned index namespace to discovery exclusions when it lies beneath a collection source directory. The operation is idempotent, leaves memory-only definitions untouched, and rejects a collection source selected from within the managed namespace. Callers must use the returned definitions for construction and reconciliation with the paired resolved storage. `resolve` hashes the same effective discovery rules when used for diagnostics. No index directory is created during preparation or policy resolution.

## Historical segment format 2 and binary manifest (superseded)

This section records the earlier binary design for context; the current contract is above.

The new magic strings are `OBJHMAN1` for the manifest and `OBJHSEG1` for a segment. Fixed-width integers use little endian; variable-length byte strings have checked `u32` lengths. File offsets and section lengths are `u64`. Segment-local record slots and posting cardinalities are `u32`, matching portable 32-bit Roaring and the existing record-slot limit. A segment with more than `u32::MAX` slots is rejected before sealing; subsequent records start another segment. No native pointer or Rust object layout is persisted.

The historical v2 binary manifest held generation, segment format version, model version, full repository and configuration identities, canonical root bytes, immutable segment names and lengths, active masks, a `validation_version` for each segment, and an optional generation catalog with length and SHA-256. That binary manifest, its independent counters, mandatory hashes, migration path and validation-stamp rewriting have been removed. The current TOML manifest is above; a lone binary manifest is only recognized as incompatible. Segment format 2 persisted an explicit `values_indexed` field-binding flag and structural facts for every declared field. The catalog lives at `generations/<generation>/catalog.bin`; its `OBJHCAT1` envelope has a 512 MiB cap. Its payload carries current file metadata, retired ID holes, version tokens, fingerprints and configuration identities, plus distinct historical file revisions referenced by segment-local metadata IDs. This keeps ordinary file metadata in one durable table. Segment names are generated by Objekte and contain no user input. An empty generation has an empty segments directory. Directory entries contain section kind, offset, length, checksum, and reserved flags; checked arithmetic establishes nonoverlap and file bounds. Empty sections have zero length. There are independently addressed sections for field/capability/coverage metadata, text bindings, scalar and array-length dictionaries, structural references, normal and skip-1 bigram dictionaries, Roaring payloads, record slots and source locators, sort columns/permutations, relation and identity lookup data, and integrity metadata.

Postings use individually addressable standard portable Roaring payloads packed in one byte region. A descriptor stores `u64` payload offset, `u32` length and `u32` cardinality. Empty postings have a distinct zero-length descriptor; absence of a field or binding is represented separately by coverage metadata. Equivalent structural facts may share a descriptor. Payloads are never compressed as a single outer stream. Builders can retain mutable maps until sealing, but the published query representation holds encoded descriptors and bytes and releases the maps. Memory-only sealing writes into owned buffers without a temporary file.

Scalar and array-length dictionaries are sorted leaf blocks, targeting 16 KiB of encoded entries. Each leaf has an offset table with one checked `u32` offset per entry, allowing binary search without decoding preceding variable-length keys. Searches decode only keys sampled by binary search, compare through the existing typed comparator, and then traverse sequentially for a range. Small dictionaries occupy one block. A key larger than the target gets a one-entry oversized block; an individual key is capped at 64 MiB. The sparse directory records each block's first and last typed key, location and entry count. The whole dictionary section has a SHA-256 checksum and each posting descriptor has a short payload checksum. Large directories can use another sparse level, but every level uses the same checked boundaries. The encoded representation preserves exact number comparisons and type distinctions; no textual numeric or unproven raw-byte ordering is used.

The typed posting adapter packs standard Roaring payloads, bulk-loads builder maps into these blocks, and returns `Result` for equality, bounded range unions and restricted lookup. Its tests compare the old B-tree operations against the sealed reader across leaf boundaries and exact decimal numbers. Managed reopen and memory-only construction install it in the production query path. Memory-only construction seals into owned buffers before reporting completion, and subsequent publications seal new segments before becoming visible.

Normal and skip-1 grams remain separate per text binding. Sparse ASCII and Unicode gram dictionaries use sorted numeric keys, a block directory and posting descriptors. An optional direct ASCII ordinal table is permitted only after measured occupancy justifies its retained cost. Sort columns persist collation keys and slot permutations so opening does not regenerate full orders. Remaining record, relation, identifier and file lookup data is sectioned and measured; any indispensable accelerator rebuilt at open is reported by size and time.

The internal gram adapter now packs both families into the shared payload region, sorts observed numeric gram keys without cloning their bitmap values, and uses descriptor cardinality to order intersections. Differential tests compare its candidates and estimates against the mutable ASCII and Unicode bigram index, including skip grams and separated text segments. Production query routing and persisted binding metadata remain to be connected.

`BigramIndex` now has an internal checked-reader installation step that releases its mutable gram maps after a validated shared segment is available. The fallible candidate/estimate methods are exercised against the installed reader. Memory-only publication installs this reader, and the deprecated snapshot writer is unavailable to production callers.

Segment FTS and text-filter candidate paths now return storage errors through the query executor, including recursive Boolean bounds and relation pruning. A focused lifecycle-segment test installs a sealed raw/fallback reader with deliberately corrupted posting bytes and verifies the FTS candidate call errors instead of treating the gram as absent. Static mutable collection access keeps the established infallible path. Memory-only publications install the sealed reader before they become visible.

## Validation and reusable evidence

Validation produces evidence keyed by immutable segment identity plus the exact section, block or payload byte span. It is shared by pinned views. A newly published segment has new evidence; evicting a decoded posting cache entry does not discard evidence for unchanged bytes. No invalid input is converted to an empty posting or a missing key.

| Invariant | Boundary and evidence |
| --- | --- |
| Manifest identity, format, completeness and referenced file bounds | Before exposing the generation, check full identities, supported manifest/segment/model versions, manifest checksum, segment names, lengths and section directories. A reader opens without the writer lock and retries a vanished generation at most three times. |
| Directory routing, binding identity and coverage | On an unstamped reopen, validate all directory entries and the first/last key of each referenced leaf, strict order across leaves, and coverage/domain metadata. A matching validator stamp proves the typed order and fence checks for the exact immutable segment; structural checks and section SHA-256 remain. |
| Leaf framing, offset table, typed order and descriptor bounds | Unstamped external bytes get a full leaf traversal at open, including entry offsets, typed order, descriptor bounds and fence keys. A stamped reopen still checks leaf framing, offset tables and descriptor bounds, but skips typed key order and fence traversal. The writer's in-process constructed path retains its separate proof. Negative lookups rely on full validation or the matching stamp. |
| Posting encoding, cardinality and record-slot domain | Before an external posting first affects an exact result or count, check its payload checksum, portable Roaring framing, cardinality and eligible-domain subset. A decoded-cache hit carries this proof. A dense, one-byte-per-dictionary-entry ordinal table retains successful proofs for uncached postings; failures remain errors and never become absences. Descriptor cardinality is only a planning estimate until validation. |
| Logical/fallback domains | Reconstruct the record classes and check corresponding domain bitmaps at open. A claimed all-logical flag is insufficient by itself. Apply the checked domain to raw, custom and fallback postings before their results can exclude candidates. |
| Integrity and construction proof | The writer validates these invariants before publication and hashes exact immutable bytes; a file reader verifies checksums for routing metadata at open and posting payloads before use. Checksums detect alteration but do not substitute for the semantic checks above. |

Full residency reads every required section into application-owned buffers and performs all required validation before `FullyResidentReady`. It does not retain a duplicate full mapping or object graph. On-demand readers map only application-owned immutable segment files and validate routed metadata at open, then posting payloads on first use. Source files continue through the existing safe readers. Source freshness is a separate reconciliation status; a structurally complete stored generation may be searchable before its source comparison finishes.

The sealed dictionary has no per-descriptor mutex or hash map. Its checked leaf offset table assigns each descriptor a stable ordinal. The dense proof table uses one byte per dictionary entry and is private to that dictionary's eligible domain. The first external access validates the payload and publishes its proof with release/acquire ordering; repeated small postings can then use the serialized intersection path without another decode. Concurrent first accesses may validate twice. A segment's decoded-posting cache retains postings of at least 64 encoded bytes, charged against at most 5% of encoded segment size and 8 MiB per segment. Cache entries also bind the exact eligible-domain owner, so a proof cannot cross from logical to fallback records. A failed validation is never cached as an empty posting. The validation table is an optimization for immutable bytes already opened by the segment reader; it does not confer trust to a different segment or dictionary.

## Publication and cleanup

Historically, the managed directory contained `store.lock`, the atomic `manifest.bin` pointer, and `generations/<16-hex-generation>/catalog.bin` plus `segments/*.idx`. That binary design wrote new files into a private `generations/.tmp-*` directory, hard-linked unchanged segments from the previous generation, renamed the completed generation directory, then atomically replaced and synced the binary manifest. Hard-link reuse, the binary manifest and validation stamps have been removed. The current contract writes every segment anew, publishes `manifest.toml`, and keeps pinned mapped readers valid after directory removal. A missing generation during open triggers a manifest reread and bounded retry; a missing file under the same current manifest is corruption.

Garbage collection runs under the writer lock after publication. It retains the current generation and seven predecessors (`RETAINED_GENERATIONS = 8`) and also waits at least 120 seconds (`MIN_RETENTION_AGE`) before deleting an older directory. This covers bursts of publications and slow opens without reader markers. At most 64 generation entries are examined per commit; excess work is deferred and reported. Only strict generation names, owned directories and known owned files are removable. Unknown entries, symlinks and foreign-owned files preserve the whole directory and produce a reclaim diagnostic. Complete directories newer than the manifest and `.tmp-*` directories are orphan candidates after the same age check. On Windows, deletion of a mapped generation may fail and is retried on a later commit. Disk use is bounded by the retained new segments plus hard-linked references to unchanged bytes, subject to the age window and deferred cleanup.

An existing flat v1 binary manifest is no longer migrated or interpreted. It reports incompatibility and is removed only as recognized obsolete cleanup once a current TOML generation has been published. A read-only load never deletes files or rebuilds.

The unified `PROGRAM_VERSION` replaced the historical independent manifest, segment, model, metadata, record and validator counters. A current segment's writer-validation flag records the validated construction outcome; there is no stamp-version compare-and-swap and no post-load metadata write. A flag never proves posting payloads; those remain checked on first use.

The historical `ManagedStore` enforced binary-manifest and segment framing, full repository/configuration identities, writer locking, file and directory sync, hard-link reuse, and narrow generation reclamation. The current store enforces TOML/segment framing, the same identities, writer locking, file and directory sync, independent generations and narrow generation reclamation. Segment descriptors now travel in each segment header; the catalog records only one aggregate expected segment count. Historical metadata references use a high-bit tag and an append-only table so adding a file cannot shift an unchanged segment's reference. A segment whose current-file reference changes is re-encoded without source reads; every segment file is fresh in a new generation. The opener checks generation, collection, ordinal, slot count and active masks against record metadata and checks active file identity and cross-segment record IDs before exposure.

`Index::open_with_storage` now opens a complete managed generation through a source-free repository owner, or builds and flushes a complete first generation when none exists. `Index::start_incremental_build_with_storage` gives interactive clients early catalogue and sealed batch publications on a fresh build, then attaches and flushes storage before reporting completion; a valid stored generation produces an immediately ready task. The library also exposes explicit `flush_storage`, `storage_status`, `attach_storage`, and coalesced background synchronization after mutations. The managed writer encodes one segment directly into an immutable file at a time; it does not retain encoded buffers for all new segments. The catalog preserves collection-wide inference reports and path diagnostics for source-free reopen. Fresh managed builds install readers from their committed generation, and subsequent segment publications seal before exposure. Managed reopen installs sealed scalar, length, and gram dictionaries without retaining duplicate mutable posting maps; on-demand and fully resident modes share query execution. Opening and query performance gates still require measurement. CLI and desktop use the shared opener. This branch is not ready to merge.

### Frontend behavior

The TUI and desktop use `objekte_cli::storage_view::describe` for the same storage labels and guidance. The core exposes typed `Access`, `Corrupt`, `Incompatible`, `Conflict`, `Resource`, and `Other` failure categories alongside raw error text. A validation stamp write error remains a warning. Memory-only views hide managed paths, generations, validation state, and storage failures. Disk synchronization and retry are explicit user actions; a failed flush does not switch storage or delete source data.

The shared reader can open mapped bytes for on-demand use or read an immutable segment fully into owned bytes. Opening checks all section framing and non-posting section integrity before exposing a generation. Full-residency opening also checks the posting region's section checksum before returning. Individual posting semantics still require the checked posting reader when accessed; complete production readiness is pending the query and lifecycle integration.

The selected-scope deprecated snapshot cleanup is implemented as a separate locked operation called after a managed generation opens or is first committed. It inspects only the two historical alphabet filenames in the selected repository `.objekte` directory, custom base, or private temporary namespace. Each candidate must be an owned regular file with the old magic and version 27 or 28. It checks all candidates before deleting any, rejects unknown/newer content and symlinks, and leaves other files untouched.

Normal library builds reject explicit legacy snapshot load, save and synchronization entry points. The legacy codec remains available to core unit tests for regression fixtures, but production callers cannot reach it. CLI and desktop removal of obsolete actions and arguments is in progress.

In disk modes, published in-memory changes accumulate in a bounded coalescing queue and are flushed as a complete generation. A failed flush retains the previous durable generation and visible pending/error state. A finite command waits for required reconciliation and flush before success. Repeated updates trigger compaction by measured segment count, retired ratio and bytes, with bounded work and retained old views accounted for.

The known obsolete files are `search-core-ascii.snapshot` and `search-core-unicode.snapshot` with `OBJSNAP1` and versions 27 or 28. Production loading and writing of those formats are removed when the replacement is complete. In a selected disk scope, recognized obsolete files are deleted under the writer lock after checking exact path, regular-file status, header and ownership; unknown, symlinked, newer and unrelated files are preserved and reported. Memory-only never scans for them. `.objekte/collections`, schemas and source files are always preserved.

## Baseline and measurement

The fetched base has one intervening commit beyond the task's inspected revision: `da8fde6847` adds detailed timing hooks and benchmark gates. On this machine, seven profile-off ASCII fresh builds had median **1,088.23 ms**; seven restores of one 132 MB old snapshot had median **678.53 ms**. The exact binary, fixture, raw trials and command outputs are retained locally under ignored `target/hybrid-index-storage/baseline/`. The full extended Musenalm campaign currently has **220 cases**; its baseline ran with both ASCII and Unicode, result caching disabled, five repetitions per case, and all cases reported `ok`. These baseline trials have not yet established full-residency, cold-I/O or inference-enabled query costs. The final campaign must pair the retained base binary with the implementation on the same fixture and query suite.

Within-leaf equality and lower/upper-bound tests must include first/middle/last blocks, absent keys, exact numbers, long strings and oversized keys. Record comparisons, bytes decoded and allocations as well as latency; measure the offset table's storage cost. If routed metadata validation makes opening too slow, change the format or validation boundary and remeasure without moving required work to the first query.

The current admission instrumentation previously included collection-wide inference copying and merging in `coordinator_ns`. The contract revision separates it as `inference_observation_ns` without changing index semantics. Three profile-on, fresh-process Musenalm runs per mode at this revision measured median total build **1,037.93 ms** with inference disabled and **2,462.69 ms** enabled. Within exclusive admission, inference observation was **0.01 ms** disabled versus **463.78 ms** enabled; segment-local inference merge was **0.017 ms** versus **420.24 ms**. Coordinator bookkeeping was **6.65 ms** versus **8.96 ms**. These are diagnostic samples on this revision, not a matched before/after speed comparison, and the inference-disabled results cannot explain inference-enabled cost. Raw JSON is retained locally under `target/hybrid-index-storage/profile-contract/`.

Decoded posting hits carry the segment descriptor, eligible-domain owner, and
construction or checked proof in their cache key. Hits use a read lock and set
an atomic recent-use bit; insertion and eviction use a bounded clock queue.
The cache cap remains unchanged. Uncached external payloads still pass through
checked decoding, including after eviction; a cache hit never proves a
different eligible domain.
