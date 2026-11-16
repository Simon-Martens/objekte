# Objekte independent stable-index prototype specification

Date: 16 November 2026  
Revision: A1 — independently saveable and openable index files; component file format 2  
Task identifier: ic-stable-index-prototype  
Integration branch: integration/ic-stable-index-prototype  
Integration worktree: /home/simon/source/objekte-integration-ic-stable-index-prototype  
Inspected master: c54486f893a57fac0b1b80ed967ab4f5a5fd26c9  
Status: implementation assignment; no prototype measurements have been made while preparing this document.

## 1. Objective and controlling architecture

Implement a feature-gated, native Rust prototype using ic-stable-structures. Establish whether independently owned, memory-resident index structures can approach the construction and query performance of master while supporting explicit snapshots, independent loading, and subsequent mutation.

The user's controlling architectural decision is:

- Each logical field index has its own storage, ownership and snapshot file.
- Each component file can also be saved and opened directly, without a generation directory, manifest, source repository or neighboring index files.
- The physical-file catalog is a separate structure.
- Each collection's record catalog is separately addressable by RecordSlot.
- A separate path B-tree resolves a path to its physical file and all associated collection/record slots.
- Independent indexes can be constructed concurrently. An individual tree has one writer.
- There is no single database tree containing every field and metadata namespace.
- The default build uses RAM only. Persistence occurs only when explicitly requested.
- A saved index is loaded as a complete memory image when that index is needed. Reopening must not rebuild the tree by reinserting its entries.

A field's value dictionary, length dictionary, structural-state postings and text representation are distinct logical indexes and may have separate files. Every file embeds the descriptor needed to interpret that index independently. A repository snapshot manifest groups those files and records their consistency; it is not required to open one file. A file must not contain unrelated fields merely distinguished by key prefixes.

Use the crate's existing balancing and allocation implementation. Do not implement another B-tree, change the crate's node format, or fork it during this first experiment.

The result is an experimental library module and benchmark executable, not a replacement production engine. Leave the default CLI, desktop application and native backend behavior unchanged. A negative performance result is useful, provided the required implementation and measurements are complete and the reason is clear.

## 2. Starting point, evidence and permitted reuse

### 2.1 Start from master

At specification preparation, master is c54486f893a57fac0b1b80ed967ab4f5a5fd26c9, whose commit restores native B-trees and memory-only operation.

Before implementation:

1. Read AGENTS.md, the complete docs/ai/README.md, this specification and relevant existing task notes.
2. Inspect the original checkout, branches and worktrees without resetting user changes.
3. Fetch the configured remote. Record the exact remote master commit.
4. Create the integration branch and sibling worktree from that master commit. If master has advanced, use its current head and record the differences affecting this design.
5. Keep a separate pristine baseline worktree at the same pinned base, or reuse an existing verified clean baseline checkout at that exact commit.
6. Keep the original checkout on its current main branch. Do not modify, merge or push master.
7. Preserve the abandoned branches. Never merge either abandoned backend branch wholesale.

The user has explicitly selected this dependency and architecture. Adding the optional ic-stable-structures dependency and implementing the modules described here does not require another dependency or architecture approval.

### 2.2 Donor revisions

| Donor | Pinned revision | Intended use |
| --- | --- | --- |
| abandoned/redb-primary-index | 09445853ccbce1cdc690a6f1c62bfe765a72e774 | Semantic fixtures, metadata definitions, exact scalar tests, source-free primitive test ideas |
| abandoned/rocksdb-bulk-construction-repair | bd8b29e90b3bc698e69dcdb1644e704f406b5c21 | Correctness oracle, coverage descriptors, benchmark reporting, useful codecs and schema fixtures |
| master | Actual pinned base; inspected c54486f893a57fac0b1b80ed967ab4f5a5fd26c9 | Primary extraction, packed bigram construction, native postings, sorting semantics, record identity and source selection |

The latest inspected RocksDB report measured 11.585 seconds versus 0.991 seconds for native construction, approximately 11.69 times slower. It reported zero construction bitmap reads, but still spent substantial time preparing and accumulating facts. Its W=4 result did not demonstrate parallel publication. These are historical measurements from that report, not measurements of this proposal.

Consequently, copying the old admission path and replacing the database calls is expressly prohibited.

Inspect these donor files selectively:

- crates/core/src/rocksdb_store/value_codec.rs
- crates/core/src/rocksdb_store/key_codec.rs and key_codec.test.rs
- crates/core/src/rocksdb_store/reference.rs
- crates/core/src/rocksdb_store/validation.rs
- crates/core/src/rocksdb_store/schema.rs
- crates/core/examples/bench_rocksdb_index_store.rs
- docs/ai/generated/rocksdb-bulk-construction-repair/repair-report.md

Prefer adapting isolated functions and fixtures. Make a reuse-manifest.md recording source revision, original path, reused responsibility and removed backend assumptions. Whole-file copying is acceptable only after review shows every included responsibility is relevant.

Do not import TransactionDB, redb, namespace dispatch, write receipts, transaction retries, pending-file database rows, database environments, cohort ownership, per-record reverse gram retention, or the old global admission coordinator. Old comments referring to redb or RocksDB must not survive in new modules.

### 2.3 Verified master construction model

Master already performs parallel preparation with privately owned BatchBuilder instances. BatchContribution contains independent posting and packed bigram contributions. The coordinator admits completed batches in order and assigns final collection slots; CollectionAdmission::admit_profiled includes serial merges.

Therefore, do not claim master already has one persistent writer thread per field. The prototype extends parallelism to independent field/index merging and stable-tree construction, while preserving deterministic slot assignment. No concurrent-write capability inside one B-tree is required.

## 3. Fixed choices and scope boundaries

| Area | Required choice |
| --- | --- |
| Dependency | Optional ic-stable-structures = "=0.7.2"; commit Cargo.lock |
| Feature | ic-stable-prototype; default disabled |
| Main module | crates/core/src/stable_index/ |
| Benchmark example | crates/core/examples/bench_ic_stable_index.rs |
| Campaign driver | bench/run_ic_stable_index_prototype.py, only if a small driver is needed |
| Native memory | One owned RefCell<Vec<u8>> per stable structure |
| Global memory manager | Do not use MemoryManager or one shared backing buffer |
| Shared memory handles | Do not use VectorMemory's Rc wrapper across worker threads |
| File-backed operation | No FileMemory, mmap, tmpfs, memory disk, or temporary payload files during construction |
| Persistence | Explicit individual component files, optionally grouped into an immutable snapshot generation |
| Load granularity | Entire requested component, copied once into its owned memory |
| Construction writers | One owner per component; multiple components run concurrently |
| Posting format | Native Roaring accumulation; compressed Roaring bytes stored once per completed posting |
| Slot domain | Existing u32 RecordSlot, local to a collection; no slot reuse |
| Number and text semantics | Reuse master's exact Number/ScalarKey comparison, normalization, segment boundaries and collation |
| First construction visibility | Publish the complete prototype after all required components are ready |
| Production integration | Deferred; expose prototype primitives and a benchmark adapter only |
| Existing snapshots | No migration or compatibility promise; use a new format identifier |
| Additional dependencies | Reuse serde, bincode, sha2, roaring and rayon already available; no new runtime or database dependency |
| Inference | Explicitly unsupported in this first prototype; reject requests enabling it |
| Concurrency during mutation | Exclusive prototype access; no simultaneous readers observing partial changes |
| Historical read snapshots | Saved immutable generations only; no live MVCC requirement |

First-file availability, incremental construction publication, production query-planner integration, GUI work, slot reuse, full compaction and bounded working sets larger than RAM are outside scope. This does not remove the required save/reopen/mutate demonstration.

There are two distinct mutation responsibilities:

1. Actual B-tree insertion, replacement and removal of known keys and posting members must work after reopen.
2. File deletion/replacement initially uses an authoritative live-slot domain and non-reused slots, as described in section 11. Physical removal of every old posting membership is deferred. Do not add a large reverse-index subsystem to this experiment.

Report this distinction prominently. Do not call logical file deletion physical index reclamation.

## 4. Module and ownership plan

Use a small set of cohesive modules:

| Module | Responsibility |
| --- | --- |
| mod.rs | Prototype types, feature-gated interface, errors and component identities |
| memory.rs | Owned memory alias/wrapper, extraction of bytes, optional diagnostic counters |
| codec.rs | Stored key/value wrappers and checked metadata encodings |
| catalog.rs | File catalog, per-collection record catalog, path lookup and identity lookup |
| posting.rs | Typed, length, state and bigram primitives |
| build.rs | Native contribution reuse, slot mapping, per-index work and finalization |
| snapshot.rs | Standalone component save/open, shared file format, manifest and explicit generation publication |
| mutation.rs | Exclusive primitive mutations and live-slot file lifecycle |
| validation.rs | Independent native comparisons and storage invariants |
| tests/ or adjacent test modules | Focused fixtures following repository conventions |

Separate sorting/relation modules only when their implementation would otherwise obscure these responsibilities. Do not build a universal storage trait framework or refactor the production Repository around the experiment.

The prototype registry contains a small descriptor for each component and one of:

- Unloaded: immutable component file location and expected identity/hash.
- Loaded: owned structure and dirty state.
- Failed: an explicit error; never interpreted as an empty index.

The registry is metadata, not a second complete index. It may use an ordinary map keyed by numeric component identifier.

Give each logical index a stable descriptor containing collection, field path segments, named binding if any, index role, logical type and relevant normalization/collation settings. Assign numeric file identifiers deterministically after sorting descriptors. Retain the full descriptor in the manifest; do not rely on a truncated hash for identity.

No source path, field string or binding name becomes a filesystem filename directly.

## 5. Component inventory and exact lookup semantics

Each row below denotes separate independently owned components. "Per field" includes named custom bindings as separate identities.

| Component | Key | Value | File scope |
| --- | --- | --- | --- |
| Physical-file catalog | FileId | Encoded file metadata | One repository-level file |
| Path lookup | Lossless root-relative PathKey | FileId plus sorted collection/slot references | One repository-level B-tree file |
| Record catalog | RecordSlot | Encoded record metadata and liveness | One file per collection |
| Record-identity lookup | RecordId | Compressed set of matching slots | One file per collection |
| Live/domain sets | Explicit domain identifier | Compressed bitmap | One file per collection |
| Typed value postings | ScalarKey wrapper | Compressed bitmap | One file per field/binding |
| Length postings | Number wrapper | Compressed bitmap | One file per field/binding |
| State postings | Explicit state identifier | Compressed bitmap | One file per field/binding |
| Normal bigrams | Numeric gram key | Compressed bitmap | One file per text binding |
| Skip bigrams | Numeric gram key | Compressed bitmap | Another file per text binding |
| Configured ascending sort | Ordered sort key plus slot tie-breaker | Empty value | One file per sort definition |
| Configured descending sort | Corresponding descending key plus slot tie-breaker | Empty value | Another file per sort definition |
| Relation occurrences | Source slot plus occurrence ordinal | Target identity and positions | One file per relation declaration |
| Relation target postings | Target collection and target record identity | Compressed source-slot bitmap | Another file per relation declaration |
| Relation state postings | Explicit relation-state identifier | Compressed bitmap | Another file per relation declaration |

Do not create relation or sort structures when the configuration does not request them. Empty configured indexes remain represented and distinguishable from unsupported or incomplete indexes.

The first prototype uses stable BTreeMap for the variable-length catalogs as well. This avoids designing a custom string/blob allocator or incorrectly putting unbounded metadata into a StableVec. Measure catalog costs separately; a later fixed-row catalog is a separate optimization decision.

A representative generation may contain these names:

| Path | Contents |
| --- | --- |
| manifest.json | Format, schema, component directory, completeness and checksums |
| files.icss | Physical-file catalog |
| paths.icss | Path-to-file-and-record references |
| c000001/records.icss | Collection 1 record catalog |
| c000001/identities.icss | Collection 1 logical identity lookup |
| c000001/domains.icss | Collection 1 live/logical/fallback domains |
| c000001/i000014-values.icss | One field's typed dictionary |
| c000001/i000015-lengths.icss | That field's length dictionary |
| c000001/i000016-states.icss | That field's structural facts |
| c000001/i000020-normal.icss | One text binding's normal grams |
| c000001/i000021-skip.icss | That text binding's skip grams |

These are separate files and separate memories, not virtual subtrees of one global file.

### 5.1 File and record identity

FileId is repository-wide. RecordSlot is collection-local. All public prototype record references therefore contain both CollectionId and RecordSlot. Never resolve a bare slot against whichever collection happens to be active.

Preserve the current RecordEntry information: logical RecordId, FileId, byte range, source revision, record class and diagnostic reference. Preserve FileEntry information including relative path, timestamps, collection membership, validity state and diagnostics. Add an explicit live flag and incarnation where needed.

Use checked slot arithmetic. Slot assignment must be deterministic across worker counts. Replacements append new slots; they do not recycle dead slots.

Path lookup must support:

- One file containing many records.
- One physical file belonging to multiple collections.
- Empty or unreadable files with zero logical records.
- Broken/raw-fallback records.
- Distinct files with duplicate logical RecordId values.

A PathEntry contains its FileId and sorted, deduplicated RecordRef values. A path lookup returns the references; record metadata supplies the corresponding logical RecordId values. Provide a convenience lookup returning these identities without scanning a catalog.

A duplicate logical identity must map to all matching slots. Do not silently overwrite one record with another.

Use the existing path identity semantics. Do not lowercase paths, perform lossy UTF-8 conversion or conflate platform separators. If a lossless portable path codec does not already exist, use a platform-tagged native path representation and reject snapshots from a different path platform. The implementation must run on Linux, macOS and Windows; portable execution does not imply that every platform-native path can be transferred between them.

### 5.2 Coverage and field semantics

Persist value_complete, length_complete, empty_string_complete, values_indexed, covered_records, field type, multi-value status and named-binding coverage.

Incomplete coverage is not an empty result. The prototype API must distinguish "no restriction available" from "complete index proves no matches." Preserve missing, null, empty-selection, empty-array/object and type-state distinctions.

Keep per-field and per-text-binding semantics intact. Do not merge raw text, fallback text, logical text and custom text into one gram tree.

### 5.3 Sorting and relations

Reuse native collation-key generation and exact comparison behavior. Do not replace numeric comparison with float conversion or decimal-string lexical order.

For a stable sort-tree key, store a wrapper containing the effective direction and null/missing placement, the native SortKey and the slot tie-breaker. All keys in one component use the same options. Implement a total comparison by comparing option identity first, then calling the existing native comparison for those options, then applying the native slot tie-breaker. Define Eq consistently with that comparison, including Missing/Unavailable equivalence where native ordering treats them equally. Test ascending, descending, equal keys and placement options against master. Do not assume descending is simply reversing ascending, because placement and tie rules may differ.

Configured composite sorts, if present in the selected fixture/profile, must preserve native constituent comparison and tie rules in their own component. Query-created inferred sort caches remain outside scope.

Relation occurrence ordinals and positions preserve multiplicity. The target posting is only a candidate set; it cannot replace the occurrence records. A target need not currently resolve. Do not rewrite dangling references into absence or collapse separate occurrences.

## 6. Memory implementation and thread transfer

ic-stable-structures 0.7.2 already implements Memory for RefCell<Vec<u8>>. Its VectorMemory alias adds Rc, which would prevent ordinary transfer between worker threads. Use the owned RefCell form directly for the first implementation.

Conceptual aliases:

    type IndexMemory = std::cell::RefCell<Vec<u8>>;
    type IndexTree<K, V> = ic_stable_structures::BTreeMap<K, V, IndexMemory>;

The library's memory allocation unit is 65,536 bytes. That is an allocation convention, not a requirement for application-level disk paging. Persist the logical memory length, including allocated free space; do not persist unused Vec capacity or trim allocator-owned bytes.

Requirements:

- One independently allocated memory per tree. Never clone/share one memory as backing for two structures.
- Transfer ownership of a complete tree or its Vec between workers; do not share mutable trees.
- Add a compile-time Send assertion for the actual component types used in the worker results.
- Do not implement unsafe Send or Sync.
- Do not add Arc<Mutex<Vec<u8>>> to every memory operation. It would serialize unnecessary accesses.
- A type being Send does not make it Sync. The prototype query adapter may run under one owning thread/exclusive access. General concurrent production queries are deferred.
- If diagnostic Memory counters are added, wrap/delegate the existing implementation without changing semantics. Keep the uninstrumented build as the timing default.
- On 64-bit native targets, lengths and allocation requests must be checked before file allocation. Report allocation failures where possible; do not claim process OOM is recoverable.

Use BTreeMap::new for fresh empty memory. For a verified saved image use BTreeMap::load, not init: init can initialize a new tree when the expected header is absent. A corrupt file must not become a new empty index.

The supported persistence path is into_memory(), extraction/borrowing of the Vec bytes, and load() over those same bytes. Do not copy the Rust BTreeMap object representation or transmute raw byte buffers into Rust objects.

Minimal ownership proof, before adding envelopes and file I/O:

    use std::cell::RefCell;
    use ic_stable_structures::BTreeMap;

    type Memory = RefCell<Vec<u8>>;
    let mut original = BTreeMap::<u64, u64, Memory>::new(RefCell::new(Vec::new()));
    original.insert(7, 11);
    let image = original.into_memory().into_inner();
    let mut restored = BTreeMap::<u64, u64, Memory>::load(RefCell::new(image));
    assert_eq!(restored.get(&7), Some(11));
    restored.insert(8, 12);
    assert_eq!(restored.remove(&7), Some(11));

The snapshot implementation transfers these image bytes, not key/value entries. The component envelope and embedded descriptor are outside this memory image and must be removed before constructing IndexMemory.

## 7. Encodings and the principal performance risk

The stable tree accesses encoded bytes even with RAM backing. Its get returns an owned V. RAM backing therefore does not mean native borrowed access or zero decoding for queries. The prototype must measure this cost instead of assuming it away.

### 7.1 Stored values

Implement a StoredBytes newtype:

- to_bytes returns a borrowed byte slice.
- into_bytes consumes the value and moves its Vec without cloning.
- from_bytes uses Cow::into_owned.
- BOUND is Unbounded.

Use this for encoded bitmap and metadata payloads. Decode through checked application functions returning Result when a caller needs their contents. Do not make Storable::from_bytes eagerly deserialize every returned bitmap, including old values returned and immediately discarded by insert/remove.

For bitmaps, use roaring 0.11 already in master, serialize_into and checked deserialize_from. Keep compressed container representation. Do not expand each posting into separate tree entries or plain lists of record IDs.

For metadata, use explicit versioned bincode settings, fixed integer encoding, little endian and trailing-byte rejection. Include bounded lengths/checks when decoding. Do not use JSON for every record or posting. JSON is appropriate for the small manifest.

Do not import contribution compression or LZ4 solely because a donor used it.

### 7.2 Stored keys

Use semantic wrappers whose Ord delegates to master's ScalarKey or Number, with round-trip encoding that preserves the exact value. Bincode bytes are not an order-preserving numeric encoding: never derive bytewise ordering over arbitrary serialized ScalarKey bytes.

For variable strings, decimal representations, paths and metadata use Unbounded. For genuinely fixed-width numeric or state keys, accurately declare the fixed bound. Do not invent small maximum string/key lengths to force bounded storage. Do not change a stored bound without changing the component codec identity.

Storable cannot return a decoding error for K. Restrict this prototype to snapshots written by this implementation, validate the envelope/hash before library access, and document that the crate is not a hardened parser for hostile validly-checksummed tree images. Catch load panics at the outer boundary where feasible, discard the affected component and return an error; this is not a substitute for structural validation or protection against OOM. Never continue with a partly loaded/mutated tree after a panic.

### 7.3 Construction write policy

Build each full posting with the current native compressed accumulation machinery. Once complete, encode it once and insert one value into the stable tree.

Prohibited construction loop:

    for every record membership:
        read current bitmap from stable tree
        decode bitmap
        insert one slot
        encode entire bitmap
        replace stable-tree value

Required pattern:

    accumulate one index using native maps/packed gram contributions
    consume the completed index
    for each distinct key:
        encode its final bitmap once
        insert its bytes into that index's stable tree
        release the native posting

Count application-level bitmap encodes and tree insert calls. During a fresh construction they must equal the number of stored posting keys, excluding explicitly separately counted domain/catalog operations. This does not claim the crate writes a node only once internally.

Do not describe sorted ordinary insert calls as a dedicated bulk-loader API. Use the supported public API; no private-layout bulk writer is authorized.

## 8. Parallel construction algorithm

The first implementation deliberately uses a simple bounded-wave scheduler. Do not introduce actor runtimes or complicated shared work queues to get the first measurement.

### 8.1 Phase A: discovery and descriptors

Reuse master's file discovery, collection selection, source revisions, adapter preparation and descriptor compilation. Record source reads and parses. Do not reread discovery metadata merely to populate a second representation.

Freeze component descriptors before final stable-tree construction. Initial inference is disabled. Build the minimal ordered batch-job inventory using existing conventions.

### 8.2 Phase B: parallel extraction and per-index accumulation

1. Prepare a bounded wave of existing batch jobs in a Rayon pool of P workers.
2. Preserve the existing per-job source-size/file limits. Limit a wave to at most 2P jobs and a configurable 64 MiB estimated preparation target, with one explicitly reported oversized-job exception.
3. Completed batches retain native field posting fragments and packed BigramContribution data. Do not flatten every gram into per-record heap vectors.
4. In deterministic job order, the coordinator assigns each collection's dense slot base using batch record counts.
5. The coordinator creates small mapping descriptors and moves each field/text/sort/relation fragment into the corresponding index's pending list. It must not iterate every posting membership.
6. Process independent index owners with parallel mutable iteration. Each owner merges only its own fragments, in assigned slot order.
7. Process catalog/path metadata as independent batched work where possible. Preserve whole-file identity across record ranges; a multi-record file may span preparation jobs.
8. Release the wave's transient ownership and continue.

A wave barrier is an explicit first-prototype simplification. It avoids deadlock between producers and consumers sharing a bounded thread pool. Measure its idle time. Do not add a nested pool of P workers per field.

All thread counts are global budgets. Use one Rayon pool and supported nested work sharing, not multiplicative oversubscription.

Existing native contribution methods may need narrow extraction or visibility changes. Keep feature-off behavior unchanged. In particular, reuse BigramContribution's compact word/chunk approach and its existing final bitmap conversion rather than routing millions of grams through the abandoned PreparedGramSet representation.

The final native accumulators are permitted transient construction state. They are not an unbounded queue. Distinguish their memory from the preparation budget; do not claim the entire index fits in 64 MiB.

### 8.3 Phase C: parallel stable-tree construction

Turn each completed logical component into one job. A job owns:

- Its native accumulator.
- Its descriptor.
- One new stable memory.
- Reusable encoding scratch and counters.

Run independent jobs through the shared P-worker pool. Consume native maps, encoded metadata and bitmaps by ownership. Release each native payload after insertion. Do not clone the entire native repository or retain both native and stable query backends after readiness.

Large dictionaries may dominate the tail. Record the largest component duration and remaining-worker utilization. Do not silently change the file model into key-range sharding to hide this limitation.

The source-build timer includes every phase: discovery, preparation, routing, merging, stable encoding/insertion, metadata construction and release of transient builders. Ending the timer before stable-tree construction is invalid.

This design may temporarily retain complete native accumulators plus partly constructed stable images. Measure peak RSS and ownership; do not call it a single-representation build. The retained result after readiness must contain only stable images and small descriptors, apart from explicitly identified tiny live-domain caches.

### 8.4 Required parallelism evidence

Record per-component start, finish, worker identifier, entry count and role in a diagnostic run. Demonstrate at least two field-index jobs overlap on a multicore fixture. Reporting workers=8 is not evidence of overlap.

The slot coordinator may assign identities serially. It may not merge all field postings or build all stable trees serially before handing out completed results.

Run P=1, P=2 and P=min(8, available_parallelism) on the same workload. Record actual available cores, affinity and resolved worker count. If only one core is available, mark parallelism unverified.

## 9. Explicit snapshots and independent loading

### 9.1 File envelope and manifest

Each component file consists of a fixed envelope, a length-delimited embedded descriptor, then its stable memory bytes. The identical file format is used for a standalone index and a component inside a generation. No sibling manifest or sidecar file is required to interpret the file. The envelope records:

- Objekte format magic and format version 2.
- Component identifier and descriptor digest.
- Structure kind and key/value codec versions.
- Pinned ic-stable-structures version and accepted internal layout version.
- Payload byte length; page count is derived as length / 65,536.
- Expected entry count.
- Payload SHA-256.
- Embedded descriptor length, format version and SHA-256.

Encode fields explicitly; do not write a Rust repr(C) header as raw bytes.

Fix the outer envelope at 128 bytes, with little-endian integers:

| Byte offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 8 | ASCII magic OBJICSS2 |
| 8 | 4 | Outer format version, 2 |
| 12 | 4 | Explicit stable component-kind code |
| 16 | 8 | Component identifier |
| 24 | 32 | SHA-256 of the exact embedded descriptor bytes |
| 56 | 4 | Key codec version |
| 60 | 4 | Value codec version |
| 64 | 6 | Three u16 values: crate major, minor, patch |
| 70 | 2 | Accepted internal tree layout version |
| 72 | 8 | Payload byte length |
| 80 | 8 | Expected entry count |
| 88 | 32 | Payload SHA-256 |
| 120 | 4 | Embedded descriptor byte length |
| 124 | 4 | Descriptor encoding version, initially 1 |

Define component-kind codes explicitly in codec.rs; never persist Rust enum discriminants implicitly. For new 0.7.2 trees expect the V2 tree layout and verify that in the minimum proof. Reject any other internal layout rather than invoking automatic legacy migration. Outer file format 2 supersedes the initial specification's format 1; do not silently open an old file as the new layout.

Encode the embedded descriptor with the explicit fixed-integer, little-endian, trailing-byte-rejecting bincode configuration already required in section 7. Use a versioned struct with a fixed field order and ordered lists. Bound its encoded length to 1 MiB in this prototype; reject an oversized descriptor before writing and before allocating during reading. The payload starts at byte 128 + descriptor length. The actual file length must equal 128 + descriptor length + payload length, with checked arithmetic. Only the payload length must be a multiple of 65,536; its file offset need not be aligned because it is copied into its own Vec.

The embedded descriptor contains:

- The complete logical index identity: collection identity, field-path segments, binding name and role, as applicable.
- Key/value logical types, codec versions, ordering options, normalization/alphabet/collation settings and their versions.
- Coverage/completeness flags and slot high-water information needed to interpret this index.
- Repository/configuration identity and a slot-namespace identifier for interpreting record references; standalone synthetic fixtures may explicitly use a detached namespace.
- Origin state/revision information when exported from a repository, and logical dependency identities such as its collection's live domain. Do not embed other indexes or catalog contents.
- Whether the handle has undergone detached standalone mutation since export. Such a mutation does not claim to update the original repository or its generation.

Assign a slot-namespace identifier at repository build, persist it across save/load, and preserve it for append-only slot allocation. Change it whenever a future rebuild/remapping changes slot identity. Matching namespace identifiers are necessary for combining record references but do not prove that independently saved indexes describe the same logical state.

During saving, encode/hash the descriptor once, write a placeholder envelope followed by the descriptor, stream/hash the payload, seek back to finish the envelope, then sync. Reuse this writer for standalone files and generation files. A reader verifies both descriptor and payload hashes and checks agreement between their type/version fields before constructing the tree.

The manifest records the repository/configuration identity, schema and source fingerprint, collection identities, slot-namespace identifier, slot high-water marks, coverage metadata, normalization/collation versions, and a sorted component directory. Each directory entry contains the relative file path, descriptor identity/hash, lengths and payload checksum. Validate any duplicated metadata against the embedded descriptor. The embedded descriptor is sufficient for standalone opening; the manifest supplies the cross-component consistency contract.

The manifest defines one complete snapshot generation. Individual component bytes need not contain the enclosing generation number, so an unchanged component can be copied byte-for-byte into another generation. Its descriptor and expected checksums bind it to the new manifest. Origin revision is provenance, not a requirement that all unchanged files be rewritten on every generation. An old, changed component substituted into a new generation must fail verification. Do not infer a coherent repository merely because independently exported files have compatible schemas or matching slot namespaces.

Use numeric safe filenames and reject absolute paths, traversal and duplicate component paths in the manifest. Validate arithmetic before allocation. Reject incompatible format/configuration/type identities rather than guessing or repairing them.

### 9.2 Save operation

Expose save_snapshot(new_generation_directory). The destination must not already exist. Return the explicit path of the completed generation.

For this prototype:

1. Require exclusive access and quiesce mutation across the entire prototype.
2. Create a unique sibling staging directory reserved for unfinished generations.
3. Write each loaded component's memory payload without a full-size clone. Recover memory using into_memory, stream/hash its bytes and restore the tree over the same allocation even if file writing fails.
4. For unchanged unloaded components, stream-copy and verify the already saved component file. Do not load or reconstruct its tree just to save another component.
5. Write the manifest after component files are complete.
6. Flush and sync component files and manifest. Close handles before publication.
7. Rename the staging directory to the previously nonexistent final generation path on the same filesystem.
8. Sync the containing directory on platforms that support it, and record platform-specific durability limitations accurately.

Use bounded file I/O parallelism, default min(4, P). Include hashing and syncing in snapshot timing. Do not promise power-loss durability beyond the OS/filesystem guarantees actually exercised.

No CURRENT pointer, automatic latest-generation selection or overwrite-in-place protocol is required. Callers explicitly open the returned generation path. This avoids adding a cross-platform pointer-replacement protocol to the first prototype.

A failed save leaves the previous completed generation unchanged. Never delete existing generations automatically. Staging directories must not be accepted by the loader.

Individual exports do not modify an existing generation's manifest or files. A later explicit save_snapshot selects the owner's current coherent components and produces another generation. Arbitrary automatic assembly of separately mutated files is outside this first prototype.

### 9.3 Open and first component access

Expose open_snapshot(generation_directory) and load_component(component_id).

open_snapshot validates the manifest and descriptor directory. It must not read all component payloads, enumerate all records or reconstruct every lookup. File existence/length checks are permitted and separately measured.

On first use of a component:

1. Validate its envelope and embedded descriptor against the manifest, using the same reader as open_index.
2. Allocate one Vec for the complete payload with checked sizing.
3. Read and hash directly into that allocation; compare the digest before library access.
4. Verify page alignment and compatible internal header.
5. Move the Vec into IndexMemory and call BTreeMap::load.
6. Check the persisted entry count against the manifest and install the loaded component.

Do not call insert while loading. Do not scan all keys to reconstruct directories or stats. The embedded descriptor supplies the component's metadata and the manifest supplies the generation directory. Full semantic validation is a separate test mode.

Read all bytes of the requested component, not just its B-tree header. Report manifest-open latency and first-component-load latency separately. Full memory residency here means that the component bytes are owned in RAM; it does not pin pages against operating-system swapping.

A query loads only its requested field/text components and small required catalog/domain dependencies. For example, field A equality must not load field B, unrelated text indexes or all source records.

No eviction policy is required. Loaded components remain resident for the prototype lifetime. Dirty components must never be silently discarded. No permanent decoded copy of every bitmap is allowed as a way to conceal stable-tree query costs.

### 9.4 Standalone index save and open

Standalone files are a required first-class interface, not a test-only shortcut or an implicit one-component repository snapshot.

Expose these operations:

| Operation | Required behavior |
| --- | --- |
| save_index(component_id, new_file_path) | Explicitly save exactly one component from an owning prototype; return its descriptor, lengths and checksums |
| open_index(file_path) | Read one self-contained file into RAM and return a StandaloneIndex handle, with no manifest discovery |
| StandaloneIndex::save_as(new_file_path) | Save that handle independently after inspection or mutation |
| StandaloneIndex::descriptor() | Return its embedded identity, codec, coverage and provenance information |

Provide a tagged handle or checked typed conversion covering the configured component kinds. Reject a requested key/value type or index role that does not match the file. Never accept an unchecked caller reinterpretation of a tree's key/value codec.

open_index validates the fixed header, bounded descriptor, file length, hashes, internal layout and entry count; then it loads the payload through BTreeMap::load. It opens only the explicitly supplied file, performs no directory scan and never searches for manifest.json. It must work after the file is copied or renamed into an otherwise empty directory, with the original repository unavailable. Filename and enclosing directory are not part of the logical type identity.

Standalone storage access supports inspection, raw lookup/range traversal, insertion, replacement and removal of known keys. It exposes physical stored postings and record references. Repository-semantic queries still require compatible dependencies such as liveness and record metadata. Make that distinction explicit in API names, for example raw_posting versus the repository adapter's live-filtered posting. A standalone file does not silently treat every stored slot as live. An operation requiring an absent dependency returns MissingDependency; basic raw tree operations do not require it.

A standalone mutation marks the handle's provenance as detached from its exporting repository state. Saving it does not update the original generation, other indexes or catalogs. Do not automatically install it into an existing repository based only on numeric component ID. Consistent assembly needs an explicitly validated owner/manifest; general import/reconciliation is deferred.

save_index captures the component and its descriptor under exclusive owner access. It must not load, encode or write unrelated components. For an unchanged unloaded component, verified streaming copy of its self-contained file is sufficient. For a loaded component, use the existing memory-image extraction/restoration path without cloning the payload.

Publish the standalone file through a unique sibling temporary file: write the envelope, descriptor and payload, finish hashes, flush/sync, close handles and rename to the requested new path on the same filesystem. Sync the parent where supported. For the first prototype, destinations must not already exist; concurrent publishers to one destination are unsupported. Fail rather than overwrite a saved generation member. Do not expose the unfinished temporary file as a completed index. Failed saves preserve the prior file and the RAM handle according to the existing ownership/error rules.

There is no background synchronization. Both an independently opened index and a generation-owned component remain RAM-resident working structures; only an explicit save operation writes their changes.

## 10. Prototype read interface and scope of query claims

Implement these primitives using the actual stable structures:

- file_by_id and record_by_slot(collection, slot).
- records_for_path and records_for_identity.
- Typed equality and bounded range over a field.
- Length equality/range and the included state predicates.
- Normal/skip gram lookup and candidate intersection using existing gram preparation.
- Ordered sort traversal/pagination for configured definitions.
- Outgoing relation occurrences and target/backlink candidate lookup.
- Explicit component load and component statistics.

Repository-semantic read results must apply the authoritative live-slot domain. The explicitly named standalone raw storage operations in section 9.4 expose physical contents instead. Complement operations must use the correct live logical/fallback domain, not all u32 slots or a stale covered-record count.

FTS primitive results are candidates. They are not verified text matches. Compare candidate generation independently and, in the fixture adapter, reuse the native source verifier when reporting verified results.

A range read must traverse the stable tree in key order and decode only visited posting payloads. It must not materialize a complete std::BTreeMap. Consume iterators by moving returned values; account for allocation inherent in the crate.

A small decoded live bitmap per accessed collection is permitted because virtually every query requires it. It must be explicitly accounted, invalidated/updated on mutation and separately reported. A complete native posting cache is not permitted.

Do not describe these primitives as full Repository API compatibility or run an incomplete query adapter against the full suite while silently scanning unsupported predicates. Mark unsupported operators/configurations explicitly.

## 11. Mutation requirements after reopen

### 11.1 Actual stable-tree mutation

On a reopened structure demonstrate:

- Insert a previously absent key.
- Replace a variable-length value with a smaller and a substantially larger value.
- Remove an existing key.
- Remove a missing key without corrupting the structure.
- Insert/remove members of a posting; remove the key when its bitmap becomes empty.
- Insert enough keys to split nodes, remove enough to exercise merging, then save and reopen again.

For one update batch, group changes by component and posting key. Decode a touched bitmap once, apply all additions/removals, encode once and replace once. Do not repeat the cycle for each member in the batch.

Known-key primitive mutation does not need reverse indexes. Test it with externally provided old/new facts.

The cost of rewriting a very large posting for one member remains real. Measure sparse, dense and high-frequency postings. Do not claim this approach provides constant-cost bitmap mutation. Chunked posting values or a mutable decoded cache would be subsequent design experiments.

### 11.2 Minimal file lifecycle

Provide fixture-level operations over prepared file records:

- Add: allocate FileId as appropriate, append collection slots, add catalog/path/identity entries and update affected indexes.
- Delete: use path lookup to obtain every affected RecordRef; mark records dead, remove the path mapping, and update the live domains and file state.
- Replace: fully prepare the replacement first, then mark old slots dead and append new slots. Update path references and all new facts.
- Move: update catalog and path lookup for a rename preserving collection membership and identities. If selection, adapters, logical identity or indexed facts depend on the path, use the replacement path or return an explicit unsupported-operation error. Never silently retain wrong identities or path-derived facts.

Deleted slots may remain physically present in value, text, sort, relation and identity postings. Every externally visible primitive filters them. Persist liveness. Old record metadata may remain as a tombstone for this prototype.

This is intentionally the simple existing lifecycle strategy. Do not build/store per-record lists of every gram during construction. Do not claim the file-to-record mapping identifies all posting keys to physically erase; it identifies the affected slots.

Report dead-slot accumulation and physical bytes after repeated replacements. Compaction and complete physical removal are deferred. Add a fixture-level estimate of this growth so it is not hidden.

### 11.3 Mutation publication and failure

All mutations run under exclusive prototype access. Prepare and validate source extraction, IDs, range checks and encoded metadata before changing existing structures. Components may be updated in parallel by separate owners, but no reader or snapshot may run until all updates finish.

There is no multi-file transaction engine. If an unexpected failure occurs after mutation has begun, mark the in-memory prototype unusable for queries/save, return the error and require rebuilding or reopening a completed generation. Do not expose half-updated state as valid. No rollback guarantee is required.

A saved older generation remains readable and unchanged after later in-memory changes and after saving another generation. Test this explicitly.

## 12. Correctness validation

Build the oracle from the pinned native master and compare semantics independently of the new codecs. Reusing an encoder for both sides does not establish correctness.

Required fixtures:

1. Empty repository, empty collection and empty configured indexes.
2. Many one-record files and a file with many records spanning preparation batches.
3. One file in multiple collections; repeated logical IDs in distinct files.
4. Malformed/unreadable files and raw-fallback records with diagnostics.
5. Exact numbers: large integers, negative values, decimal forms that compare equal, very small/large exponents and boundary ordering.
6. Unicode paths and text, empty strings, strings with separator-like bytes, and non-UTF-8 paths on supported native platforms.
7. Null, absent, empty selection, empty array/object and mixed array states.
8. Declared fields and named custom bindings sharing a path.
9. Separate raw/fallback/custom text bindings, ASCII/Unicode alphabets, normal/skip grams and segment boundaries.
10. Equal sort keys, ascending/descending order, missing/null placement and configured composites when supported by the profile.
11. Dangling relations, duplicate occurrences and distinct position vectors.
12. Very high-cardinality singleton postings and very frequent dense postings.
13. Independently saved components larger than one memory allocation page.
14. Add/delete/replace/move, followed by a second save and two independent reopens.

Validate complete membership equality and exact metadata, not just counts. Validate actual forward sort/relation structures; do not reconstruct the candidate result from an unrelated reverse representation.

Persistence tests must establish:

- Save one field index directly without creating a snapshot directory or manifest; open it in another process and compare its exact raw contents.
- Copy/rename that one file into an empty directory, remove access to the original repository, and open/query/mutate/save_as/reopen it successfully.
- Apply the same standalone save/open round trip to record catalog, file catalog and path lookup files, including multiple records per path.
- Open a file from inside a complete generation directly with open_index while its manifest is unavailable; require the same storage contents as generation-based loading.
- save_index reads/writes only the selected component, including the unchanged-unloaded copy path.
- Missing standalone liveness/metadata dependencies produce MissingDependency for repository-semantic operations, while raw operations still work.
- A detached mutation remains visibly detached after save/reopen and is not automatically installed into its origin generation.
- Unsupported descriptor versions, truncated descriptors, descriptor-hash mismatches, oversized descriptor lengths and requested type mismatches are rejected before tree use.
- Save, discard every live object, then reopen in a new process.
- Query one field and prove unrelated component payloads were not read.
- Query another field and prove only its dependencies were additionally read.
- All-component load contains no insertion calls.
- Snapshot without source files present still supports catalog/index primitives; verified source scanning is a separately unsupported operation in that fixture.
- Mutation after reopen, second snapshot and unchanged first generation.
- Save with an unchanged unloaded component does not decode/load it.
- Truncated file, changed payload byte, wrong checksum, wrong component identity, unsupported format, missing file, malformed manifest and swapped changed component all fail explicitly.
- Faults before publication leave the previous generation usable; incomplete staging directories are rejected.
- Failure during snapshot writing restores usable RAM ownership or reports an explicit invalid state, never drops the only index silently.

Use fault injection around write/flush/rename boundaries, not destructive machine power tests. Checksums detect accidental corruption; they are not authenticity signatures.

For deterministic fixtures compare outputs across P=1, P=2 and P>2. Component byte-for-byte equality is optional because insertion order can alter physical layout, but slot assignment, descriptors, semantic results and snapshot validity must agree.

## 13. Benchmark protocol and measurements

### 13.1 Fair baseline

Run the native baseline from the pinned master in a separate process and build directory. Use the same corpus, collection definitions, schemas, alphabet, source selection, worker budget and release settings.

Use the current Musenalm loader/support code, not an invented reduced schema. Record a digest of actual schema files and all registrations. If an unsupported schema feature is encountered, fail explicitly or select a separately named supported fixture and run both implementations with that identical fixture. A reduced-scope run cannot establish full-workload parity.

The historical approximately one-second build is context only. Remeasure on the implementation machine. Do not hard-code it as a performance denominator.

Compile before timing. Run the executable directly. Performance runs do not include a correctness scan, reference export or all-query warmup before readiness/RSS sampling.

### 13.2 Timing boundaries

Measure and report separately:

- Discovery and descriptor preparation.
- Extraction worker sum and elapsed interval.
- Coordinator mapping/routing.
- Per-index native merge work.
- Roaring encoding.
- Stable-tree insertion/construction.
- Catalog/path/identity construction.
- Total source-build time to usable stable prototype.
- Manifest open.
- Standalone index save/open, including embedded-descriptor validation and complete payload I/O.
- First load of one field with and without its catalog/domain dependencies already resident.
- All-component load.
- Snapshot payload writing, hashing, syncing and total publication.
- Point/range/FTS/sort/relation primitives.
- Known-key mutation batches and file lifecycle batches.

Elapsed intervals overlap; never sum worker elapsed times and present them as total wall time or engine CPU.

At readiness capture resident RSS and process high-water RSS before validation. Record process CPU time. Then measure after dropping the prototype to identify allocator retention if useful, without substituting that value for readiness memory.

### 13.3 Structural counters

Record at least:

- Physical files, records and source bytes.
- Source reads/parses and failed parses.
- Number of components by family and empty components.
- Distinct posting keys, memberships and bitmap encoding count.
- Stable insert/get/remove counts by build/load/query/mutation phase.
- Total logical memory bytes, Vec capacities and serialized payload bytes.
- Per-component entry count, payload size and build duration.
- Temporary native accumulator, preparation and scratch estimates.
- Snapshot files opened/read, and payload bytes read per lazy query.
- Live/dead slots and snapshot growth after replacement.
- Per-component overlap and coordinator/worker idle time in a diagnostic run.

No host index-payload file may be created before save_snapshot. Corpus reads, executable files, benchmark result JSON and normal process facilities are not violations. Check dedicated output directories and use available file-access tracing when practical.

### 13.4 Required runs

First run a deterministic synthetic fixture covering at least eight independent indexes, 100,000 distinct scalar keys in one high-cardinality index, one million aggregate memberships, a frequent gram and variable-length catalog entries. Keep fixture generation outside measured backend work.

Then run the actual Musenalm profile in ASCII with P=1, P=2 and P=min(8, cores). Use one exploratory sample per configuration, then at least five alternating fresh-process baseline/candidate samples for the selected matched worker configuration. Report medians, range and ratios. If the implementation remains viable, repeat the representative Unicode comparison with at least three pairs.

Warm filesystem-cache source builds are acceptable if both sides use the same policy and it is reported. A new process does not imply a cold OS page cache. Do not flush system caches without an established permitted benchmark procedure.

Query measurements use identical input keys, output consumption and liveness handling. Separate first-load cost from resident repeated queries. Report median and p95 with enough repetitions for stable measurements; short operations should be batched to amortize timing overhead. A candidate lookup returning owned bytes must be compared with a native operation producing the same semantic result, while also reporting the additional ownership/decoding cost.

Mutation cases: singleton and common posting changes, batches of 1/100/1,000 records, delete a multi-record file, replacement and save/reopen. Measure read/encode/write amplification.

### 13.5 Gates and bounded stopping rule

The practical target is parity with native, not merely a large speedup over RocksDB.

For the matched full workload:

| Measurement | Target / decision |
| --- | --- |
| Source-build median | Target <= native; <= 1.10x is provisionally near parity, subject to variance |
| Resident RSS | <= 1.40x native |
| Peak RSS | <= 1.60x native |
| Resident primitive queries | Report each family; a repeated > 2x material regression blocks a general parity claim |
| Reopen | No reinsertion/reconstruction; measure all byte I/O and checksum cost |
| Parallelism | Demonstrated overlap of independent field jobs, not only configured threads |
| Correctness | No semantic differences |

These are experiment criteria, not a promise that this crate can meet them.

If the full build is above 2x native in two matched exploratory pairs, or peak RSS is above 2x, inspect stage data and permit one localized repair of an obvious implementation mistake. Examples: a full buffer clone, serial field loop, discarded packed gram representation, repeated bitmap encoding or an accidentally enabled debug build.

Do not spend the experiment on another database rewrite, a custom node cache, a crate fork, a private bulk loader, a general actor scheduler or a reverse-contribution subsystem. If the result remains clearly outside the gates, complete tests for implemented behavior and report a measured rejection.

The tiny standalone save/open/mutate and generation lazy-load proofs are mandatory before the full-corpus performance gate. An early performance rejection may stop the extended Unicode/performance campaign and optional fixtures, but cannot excuse missing component separation, broken persistence, manifest-dependent standalone files, no real parallel work or omitted core families in the claimed comparison.

After a Phase 3 performance rejection, still complete the small Phase 4 read/mutation correctness cases for every implemented family and the save/reopen/failure tests. Skip the extended performance campaign, not validation of delivered code. The final report must distinguish this reduced campaign from a fully measured near-parity outcome.

Results between 1.10x and 2x construction remain a failure to establish parity, even if further investigation looks plausible. Label the result accordingly; do not rename the acceptance threshold after seeing measurements.

## 14. Implementation sequence and commits

### Phase 0 — freeze sources and isolate work

Record master/donors, create the integration and baseline worktrees, confirm corpus/schema availability and define source provenance. Add the optional dependency and feature skeleton.

Suggested commit: "Define independent stable-index prototype and source provenance"

### Phase 1 — minimum storage proof

Implement owned memory, StoredBytes, one numeric-key tree, one variable-length-key tree, self-contained component files and the manifest. First build two independent trees and demonstrate separate save_index/open_index round trips without any manifest, including mutation and save_as. Then demonstrate generation lazy loading, independent opening of a generation member and a second generation. Add actual Send/parallel transfer tests.

Suggested commit: "Add independent memory images and explicit snapshot loading"

Do not proceed while this proof is incorrect. This phase must not require the source adapter, full catalog or database donor admission code.

### Phase 2 — catalogs and semantic codecs

Implement file/record/path/identity/domain structures, exact scalar wrappers, field descriptors and relevant fixture comparisons. Finish multiple-record/multiple-collection path semantics and non-reused slots.

Suggested commit: "Add stable catalogs and exact index codecs"

### Phase 3 — direct construction from native contributions

Expose narrowly reusable native batch helpers, implement deterministic slot mapping, parallel field accumulation and parallel stable construction. Include configured typed/state/length/FTS/sort/relation families for the chosen benchmark. Ensure transient native accumulators are released at readiness.

Suggested commit: "Build independent stable indexes from native batch contributions"

Run the first complete representative source-build comparison now. Record actual field-job overlap. Apply the bounded stopping rule rather than postponing performance evidence until a production migration exists.

### Phase 4 — read and mutation evidence

Complete the scoped primitives, live-slot lifecycle, coverage behavior, source-free reopen validation and known-key physical mutation tests. Run matched primitive and snapshot measurements as allowed by the gate.

Suggested commit: "Validate stable-index reads mutations and snapshot generations"

### Phase 5 — report and deliver

Run required checks, review every production-path change, generate machine-readable results and write the final decision. Append the implementation report and follow docs/ai/README.md lifecycle rules.

Suggested commit: "Report independent stable-index prototype feasibility"

Push immediately after every commit. Leave the integration branch/worktree for the user's review and merge. Do not merge into master.

A single implementation agent may supervise this prototype. If delegating, assign disjoint modules in sibling subtask worktrees after Phase 1 establishes interfaces. The integration supervisor alone changes the integration worktree. Construction and storage work must share the fixed descriptors and codec contracts; do not allow independent incompatible architectures.

## 15. Commands, outputs and validation

Expose these example modes with strict argument validation:

- source-build: native sources to ready stable indexes; optional explicit snapshot output.
- fixture: deterministic semantic/storage fixtures.
- validate: compare all implemented facts against an independently produced native reference.
- open: manifest-only open, then requested component loads.
- open-index: open exactly --index-input as a standalone file, without reading a manifest or neighboring files.
- save-index: export exactly one selected component or resave a standalone handle to --index-output.
- primitives: matched resident or first-load query operations.
- mutations: prepared fixture changes, second snapshot and comparison.
- snapshot: explicit save, with separate timing from source-build.

Required arguments/options as applicable:

- --root PATH
- --alphabet ascii|unicode
- --build-workers auto|N
- --result-dir PATH
- --snapshot-output NEW_GENERATION_PATH
- --snapshot-input GENERATION_PATH
- --index-input INDEX_FILE
- --index-output NEW_INDEX_FILE
- --component COMPONENT_ID, repeatable for open/primitives
- --reference PATH for validate

Do not invent a --storage file mode that operates directly against files. Both fresh and loaded structures operate in owned RAM. Inference options must be rejected explicitly.

For save-index, require either --snapshot-input plus exactly one --component, or --index-input; reject both input forms together. Both require --index-output. The library save_index API must also work directly on a freshly constructed in-memory prototype, without first saving a generation. The fixture mode must exercise that path. open-index requires --index-input and must not require --snapshot-input. A standalone mutation fixture uses --index-input and --index-output and saves only after an explicit request.

Illustrative build commands:

    cargo build --release -p objekte-search-core --example bench_build_musenalm
    cargo build --release -p objekte-search-core --features ic-stable-prototype --example bench_ic_stable_index

Use the baseline checkout's executable and target directory for baseline runs. Example candidate invocation:

    target/release/examples/bench_ic_stable_index \
      --mode source-build \
      --root /absolute/path/to/musenalm \
      --alphabet ascii \
      --build-workers 8 \
      --result-dir bench/results/ic-stable-index-prototype/ascii-p8

The worker count is illustrative; apply the recorded machine budget.

Required checks against the combined integration result:

    cargo check --workspace --all-targets
    cargo test --workspace
    cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
    cargo test -p objekte-search-core --features ic-stable-prototype
    cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
    cargo test --workspace --features objekte-search-core/ic-stable-prototype

Mark the example's required-features so feature-off all-target checks remain valid. Follow repository formatting/lint procedures on changed paths. Required checks that cannot execute must be reported with the exact failure, not marked passed.

Use Linux measurements on the available benchmark host. Add or run native-target compile/smoke coverage for macOS and Windows where infrastructure permits. If those hosts/targets are unavailable, report the absence; source-level portability is not proof of tested platform behavior.

Machine-readable outputs:

- campaign-manifest.json: SHAs, executable/lockfile hashes, environment, schema/corpus/configuration digests.
- build.json: stage timing, readiness boundary and structural counters.
- memory.json: RSS/HWM, owned-memory and transient estimates.
- components.json: descriptors, entry counts, sizes, durations and optional job trace.
- persistence.json: standalone save/open and generation write/open/load timings, bytes and actual file/component access.
- primitives.json and mutations.json.
- validation.json: complete fact-family comparisons and corruption/failure tests.
- comparison.json: baseline/candidate ratios, missing evidence and gate classification.

Missing measurements are unavailable/null with explanations, never zero. Reject comparisons with different manifests, failed correctness or omitted required families.

Keep reports under docs/ai/generated/ic-stable-index-prototype/. Keep raw runs under bench/results/ic-stable-index-prototype/ following existing repository conventions. Do not commit corpus copies, compiled binaries or generated index images.

## 16. Delivery checklist and final decision

The report must answer:

1. What exact master/donor code was used, and which donor mechanisms were deliberately excluded?
2. Does each index have an independent memory and file, including catalog and path lookup?
3. Are distinct field jobs actually concurrent, and what serial work remains?
4. Does the prototype retain compact native construction and encode each fresh posting once?
5. How much of total build time is stable-tree insertion, catalog creation, routing, bitmap encoding or native accumulation?
6. What are end-to-end build, resident query and memory ratios against the matched native run?
7. Can one index file be saved/opened without a manifest, and does generation-based loading reuse the same file format and reader without reinsertion?
8. Can a reopened tree really insert, replace and remove entries?
9. What are the measured costs of full-posting mutation, and what remains physical garbage after file replacement?
10. Does one manifest bind every component to a consistent snapshot, do independent exports preserve origin generations, and do old generations survive failed/new saves?
11. Which platforms were actually tested?
12. Is the result near native parity, clearly unsuitable, or limited by missing evidence?

Use one of: near-parity prototype, performance rejection, or incomplete evidence. Explain any mixed outcome, such as acceptable build performance but unacceptable resident queries.

A correct negative experiment can be completed under this specification. Missing mandatory functionality is not a performance rejection. Preserve its evidence and branch without initiating another repair campaign.

No production migration, removal of native indexes, or master merge is authorized by completing this prototype.

## 17. Primary references

Repository sources inspected for this specification:

- [Pinned master](https://github.com/Simon-Martens/objekte/tree/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9)
- [Task workflow](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/docs/ai/README.md)
- [Native batch construction](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/build.rs)
- [Native typed postings](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/posting.rs)
- [Native packed bigrams](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/bigram.rs)
- [Native catalog and record ownership](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/inventory.rs)
- [Native sorting](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/src/sorting.rs)
- [Native benchmark](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/examples/bench_build_musenalm.rs)
- [Abandoned RocksDB donor](https://github.com/Simon-Martens/objekte/tree/bd8b29e90b3bc698e69dcdb1644e704f406b5c21)
- [Latest RocksDB repair report](https://github.com/Simon-Martens/objekte/blob/bd8b29e90b3bc698e69dcdb1644e704f406b5c21/docs/ai/generated/rocksdb-bulk-construction-repair/repair-report.md)

Crate interfaces and implementation:

- [ic-stable-structures 0.7.2](https://docs.rs/ic-stable-structures/0.7.2/ic_stable_structures/)
- [BTreeMap API](https://docs.rs/ic-stable-structures/0.7.2/ic_stable_structures/btreemap/struct.BTreeMap.html)
- [Memory API](https://docs.rs/ic-stable-structures/0.7.2/ic_stable_structures/trait.Memory.html)
- [Storable API](https://docs.rs/ic-stable-structures/0.7.2/ic_stable_structures/storable/trait.Storable.html)
- [Vector memory implementation](https://docs.rs/ic-stable-structures/0.7.2/src/ic_stable_structures/vec_mem.rs.html)
- [Tree load/get/insert implementation](https://docs.rs/ic-stable-structures/0.7.2/src/ic_stable_structures/btreemap.rs.html)

The component layout, scheduler, gates and prototype scope are engineering decisions in this specification. They are not performance guarantees made by the crate.


## Implementation report

Task identifier: `ic-stable-index-prototype`  
Completion date: 16 November 2026  
Base commit: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Integration branch: `integration/ic-stable-index-prototype`  
Integration worktree: `/home/simon/source/objekte-integration-ic-stable-index-prototype`

### Result

Completed the feature-gated prototype and its evidence campaign. The measured
decision is a **performance rejection**: the complete direct source build is
2.92x native on the matched Musenalm P=8 run, and resident/peak memory are
1.92x/2.14x native. Correctness, independent files, manifest-free standalone
opening, lazy generation loading, mutation, fault handling and parallelism were
validated. No production migration, default-backend change or master merge is
implied.

### Implemented changes

- Optional dependency `ic-stable-structures = "=0.7.2"` behind the disabled-by-
  default `ic-stable-prototype` feature.
- Independent owned memory and files for every catalog, field/binding posting,
  text gram column, configured sort and relation family.
- Direct source build from native discovery and batch contributions; no
  production `Repository` is constructed by the benchmark path. Accumulation
  still uses master's exact native posting/bigram/sort/relation structures.
- Self-contained 128-byte component envelope, bounded descriptor, descriptor
  and payload SHA-256, entry count, pinned crate/parser/layout identity, and V2
  tree layout rejection.
- Manifest generations, lazy `BTreeMap::load`, unchanged-unloaded stream copy,
  bounded parallel component publication, staging/rename, sync and old-generation
  preservation.
- Standalone save/open/save-as, checked component-key kinds, raw mutation and
  explicit missing-dependency errors for repository-semantic standalone use.
- Known-key posting mutation, file append/delete/replace/move, live-domain
  tombstoning and dead-slot/payload growth reporting.
- Descriptor dependency identities, explicit failed-component state, build
  component traces, worker/overlap evidence and source/stage counters.
- Deterministic component descriptor ordering and IDs; native record-ID
  tie-breaking for equal configured sort keys.
- Feature-gated benchmark and synthetic-fixture executables.

Production CLI, desktop and default native backend behavior are unchanged.

### Verification

All required commands passed in the integration worktree:

- `cargo check --workspace --all-targets`
- `cargo test --workspace`
- `cargo check -p objekte-search-core --all-targets --features ic-stable-prototype`
- `cargo test -p objekte-search-core --features ic-stable-prototype`
- `cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling`
- `cargo test --workspace --features objekte-search-core/ic-stable-prototype`

35 focused prototype tests cover the required semantic, lifecycle, persistence,
corruption, fault-injection, worker-determinism, source-free and direct-build
cases. Full Musenalm direct-build validation passed with zero errors.

### Benchmark evidence

See `docs/ai/generated/ic-stable-index-prototype/prototype-report.md`,
`comparison.json` and raw JSON under
`bench/results/ic-stable-index-prototype/ascii-p8-direct/`.

Required synthetic fixture: 100 files / 100,000 records / 100,000 distinct
keys / 1,000,000 tag memberships / 40 components. P8 stable 1.769 s versus
native 0.762 s (2.32x); validation passed.

Musenalm P8 median over five alternating samples: stable 4.959 s versus native
1.699 s (2.92x). Resident RSS 763.2 MB versus 397.7 MB (1.92x); peak RSS
880.0 MB versus 410.6 MB (2.14x). The stable component stage alone took 3.179 s.
P1/P2/P8 exploratory ratios were 2.31x/2.10x/2.92x.

Parallel snapshot publication took 0.326 s for 1,323 components / ~409 MB;
manifest open 27 ms; first 27.9 MB component load 26.7 ms; standalone
open/save about 28/66 ms. Lifecycle testing produced 2,000 dead slots after ten
replacements of a 200-record file, with payload growth from 1.18 MB to 1.57 MB.

The extended Unicode performance campaign was skipped after the clear
construction rejection, as permitted by the stopping rule. Platform execution
was Linux only.

### Implementation commits

- `fd4570479170` — independent memory images, explicit snapshots, catalogs,
  semantic codecs, construction, reads, mutation and focused tests.
- `ab7d06ad9c97` — lifecycle/snapshot/mutation validation and initial evidence.
- `0e9f33a98ad6` — initial completed task report.
- A later commit completes direct batch construction, synthetic/fault evidence,
  corrected memory measurements and this report.

### Remaining limitations

1. Configured composite sorts are rejected explicitly; the selected Musenalm and
   synthetic profiles contain none.
2. Memory fails the gate because native accumulators remain alive while stable
   images are constructed. Streaming per-family ownership transfer is the next
   design experiment.
3. File deletion is logical; dead slots and old posting bytes are not physically
   reclaimed. Compaction is deferred.
4. Mutation and snapshots require exclusive prototype access; no live MVCC.
5. Only Linux was executed. Windows/macOS host coverage was unavailable; path
   keys are platform tagged.
6. FTS primitives expose candidates; a verified FTS adapter and general
   Repository API parity are outside this prototype.

### Cleanup

No subordinate worktrees were created. The integration branch/worktree and the
pinned baseline worktree are retained for review and reproduction. Temporary
task memory was removed after the completed report was committed and pushed.
