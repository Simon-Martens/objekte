# Objekte Direct Snapshot Access Specification

Date: 2026-11-13  
Target: `integration/hybrid-index-storage`  
Reviewed source: `e397f8f3046926710e3e75b144d26279e8d4eb83`  
Status: implementation and measurement required

## 1. Objective

Make the persisted snapshot the usable index representation for disk-backed operation. Opening must install readers over persisted lookup structures instead of rebuilding equivalent maps, trees, membership lists, and gram-presence filters from metadata.

The original architecture is an immutable ordered dictionary with independently addressable blocks and a small routing directory: a read structure similar to a bulk-loaded B+tree. Keep that approach and Roaring postings. Extend direct access to the metadata and lookup structures that currently prevent startup from benefiting fully.

This task has two required outcomes:

1. Eliminate duplicate traversals, particularly the dictionary traversal used to construct gram-presence filters after dictionary validation.
2. Persist directly usable lookup structures and remove their reconstruction during snapshot opening.

This is a targeted continuation of the hybrid storage work. It authorizes the necessary storage-format revision and internal accessor changes. It does not authorize a new query language, new storage modes, or a general database rewrite.

### Priorities and existing obligations

Smooth repeated queries remain the highest performance priority. Improve startup without materially worsening construction, first-query latency, or update/flush costs. A few percentage points of construction improvement are useful; do not trade them away casually.

A bounded increase in stored size or resident memory is acceptable when it buys useful speed. Explain allocations, page overhead, and retained capacity; do not reject a change solely because an older RSS percentage limit is exceeded.

Continue the unfinished correctness and performance obligations in:

- `objekte_hybrid_index_storage_repair_specification.md`
- `objekte_hybrid_index_storage_remaining_inefficiencies_specification.md`
- `objekte_hybrid_index_allocation_and_query_performance_specification.md`

This specification extends the preceding allocation specification where storage-format and metadata-access changes are necessary. Preserve its completed optimizations, particularly deferred persistence-only metadata and allocation reuse.

## 2. Repository workflow

Read `AGENTS.md`, `docs/ai/README.md`, the current task, and implementation notes before editing. Record the actual starting commit, branch, and uncommitted changes. Reconcile the findings below against current code; the reviewed SHA is a reference, not an instruction to reset newer work.

Continue on the existing hybrid integration branch or a subordinate branch integrated into it. Keep the primary checkout on master and all implementation in separate worktrees under `/home/simon/source`. Preserve unrelated and user-provided uncommitted files. Follow the repository's commit, push, task-completion, and report conventions.

Do not merge into master. Deliver one integration branch for the user to review.

If delegation is useful, separate gram-filter persistence, metadata/catalog readers, and verification after the format interfaces are agreed. One integrator owns format constants, publication changes, and the final benchmark. Do not let concurrent agents invent incompatible on-disk layouts.

## 3. Findings at the reviewed revision

| Area | Existing work during opening | Required change |
| --- | --- | --- |
| Bigram dictionaries | Ordered dictionary opening validates entries; `GramPresence::from_dictionary()` then visits the dictionary again. | Construct filters while writing dictionary keys; open persisted filter bytes directly. |
| Record metadata | `decode_record_slots()` builds a file lookup and per-file slot lists. `validate_records()` traverses records again and reconstructs equivalent structures, ID lookup, and domains. | Persist the lookup structures and memberships. Validate borrowed records without constructing their runtime replacements. |
| Generation catalog | Decode materializes entries and sorts paths to detect duplicates. | Persist ordered path lookup structures with the required comparison semantics. Avoid startup sorting. |
| Catalog installation | `FileCatalog::restored()` constructs changed-file maps, changed-path maps, an ordered path tree, and active state from the decoded catalog. | Install an immutable persisted catalog base; keep only actual subsequent changes in the mutable overlay. |
| Generation validation | `restore_generation()` scans active records and constructs a cloned-ID set to check cross-segment consistency. | Preserve these checks. Reuse persisted identity ordering and streaming validation where possible; do not disguise validation scratch as an essential lookup reconstruction. |

The last row is a correctness obligation, not permission to remove uniqueness or revision checks. Merely storing an index does not prove its consistency with the record table.

Source references are listed in section 12.

## 4. Required representation

### 4.1 One logical access interface, several backing stores

Provide internal accessors for record-by-slot, file metadata, record-ID lookup, per-file membership, path lookup/ranges, and record domains. They must support:

- Existing native constructed state for memory-only operation.
- Immutable snapshot bytes in mapped storage for on-demand operation.
- The same immutable snapshot format backed by resident bytes for fully resident operation.

Share lookup and comparison logic between mapped and resident snapshots. Fully resident must not mean reconstructing every Rust collection.

Borrow strings, paths, and fixed-width values from the snapshot where their API permits it. Decode bounded values when necessary. Allocate an owned record or path at an ownership boundary, such as an external API result, rather than for every row during opening.

A reader must retain the immutable backing owner for every borrowed view. Avoid self-referential ownership, unchecked pointer casts, and references whose mapping can be retired while a query uses them. Preserve pinned-generation behavior.

Do not leave an adapter that silently collects the entire snapshot into `Vec<RecordEntry>`, `HashMap`, or `BTreeMap` on its first ordinary query or update. Audit callers, not just the open function.

### 4.2 Persisted structures

Use the existing ordered-block machinery where suitable. Dense identifiers should use indexed tables rather than a tree when that gives simpler constant-time access.

| Lookup or operation | Persisted representation | Access behavior |
| --- | --- | --- |
| Record slot → record | Fixed-width record rows, or a block directory plus row-offset table; variable identifiers and diagnostics use checked arena references. | Locate one row without decoding preceding records. |
| Record ID → slot within a segment | Ordered blocked dictionary using the existing ID comparator. Reuse existing persisted identity ordering if it provides the same lookup efficiently. | Exact lookup and ordered iteration without rebuilding an ID map. |
| Segment file ID → file descriptor | Dense table where IDs are suitably dense, otherwise an ordered blocked dictionary. | Resolve metadata reference and membership descriptor directly. |
| File → record slots | Persisted offset/count descriptors into slot-list blocks; use a range only when contiguity is guaranteed. | Read only that file's membership; support multiple, noncontiguous records per file. |
| Logical and fallback domains | Persisted portable Roaring descriptors or an established equivalent direct representation. | Avoid deriving domains by scanning record classifications during opening. |
| Generation file ID → current state | Indexed catalog slots with explicit retired slots, metadata references, revision/configuration fields, and required liveness information. | Preserve file IDs and holes; resolve current state without constructing changed-file maps. |
| Path → file ID; ordered path predicates | Ordered blocked path dictionary or separate exact/ordered indexes if their existing comparators differ. | Exact and range access without startup sorting or rebuilding a path tree. |
| Historical metadata reference → entry | Offset-indexed immutable metadata table. | Preserve old segment references without cloning the historical catalog. |
| Gram-presence test | Persisted filter descriptor and bit array bound to its dictionary. | Test the mapped/resident bytes directly. |

Catalog file versions, fingerprints, and configuration identity must remain available through this representation. Do not reconstruct another repository-wide table merely to satisfy an existing field type.

Preserve native path round trips, including supported non-UTF-8 paths, and the existing `@file.path` query ordering. Native path identity and a normalized query key are not automatically interchangeable. Likewise, bytewise string comparison must not replace established typed or collation ordering.

Domains and active masks are different: immutable logical/fallback membership must not absorb mutable liveness. Preserve their intersection semantics.

Reusing an already persisted identity-order array is preferable to storing another equivalent index. If an additional directory is necessary, explain the different operation it enables.

### 4.3 Blocks and format contract

Specify field widths, byte order, counts, offsets, sentinel values, alignment, comparator identity, and limits in repository documentation before implementing the decoder. Use checked arithmetic and bounds before constructing slices.

The format must not contain Rust collection headers, process addresses, `usize`-dependent layouts, or process-random hash state. Do not dump a native `HashMap` and call its later deserialization direct access.

Use bounded independently addressable blocks. Reuse existing block sizes initially and measure before tuning. Do not align every small entry or posting to an operating-system page.

Keep directories small. Add another directory level if the number of leaves makes a single resident directory excessive. Variable-length arenas must also be addressable without decoding their entire contents. Small schemas and binding tables may remain eagerly decoded when measured and documented.

Separate locating bytes from verifying an entire section. If the existing section accessor computes a whole-section hash, invoking it still touches the whole section even when the result is a borrowed slice. Account for that explicitly in section 7.

## 5. Work package A: persist gram-presence filters

### Writer

1. Determine filter sizing from an existing entry count or writer metadata. Do not count by traversing the dictionary first.
2. Feed each key into the filter during the dictionary writer's existing ordered input pass.
3. Finalize and persist the filter alongside the corresponding dictionary.
4. Use the same filter result for newly constructed sealed readers. Avoid rebuilding it by reopening and visiting the completed dictionary.
5. Preserve the existing small-dictionary policy where a filter is not worthwhile.

Persist enough information to interpret and bind the filter: format/hash version, hash parameters, bit-array length, key count, and dictionary identity. Binding may be established by the containing text-binding descriptor rather than duplicating all configuration fields. It must distinguish normal/skip dictionaries, alphabet, text binding, and segment.

### Reader

Open a checked borrowed filter view. Neither `open()` nor `from_constructed()` may call `GramPresence::from_dictionary()` as the normal production path.

A filter may prove absence only after its integrity, parameters, and dictionary binding have been established. False positives are acceptable; false negatives are not. A missing optional filter means exact dictionary lookup. A malformed persisted filter follows the existing typed corruption/recovery policy; never interpret it as an empty filter.

The writer must guarantee that every dictionary key was inserted. Checksums alone do not establish that semantic property. Preserve any required external consistency validation by checking coverage within the existing dictionary-validation pass, without rebuilding the filter or adding another traversal.

If filters are needed for a dictionary that is not being rewritten, reuse its existing immutable filter. Maintenance must not rescan unchanged dictionaries just to regenerate filters.

**Acceptance:** zero dictionary-entry visits solely to construct gram filters during opening; no full dictionary rescan to create a filter after construction.

## 6. Work package B: replace metadata reconstruction

### Segment installation

Replace the reconstructing `CheckedRecordSlots` installation path with a checked reader over the persisted tables. Install handles, counts, roots, and bounded validation state.

Remove open-time construction of `file_index`, `id_lookup`, and per-file `records` lists when their persisted equivalents are present. Read logical/fallback domains from their persisted descriptors. A bounded decoded domain bitmap is acceptable if required by existing Roaring operations; rebuilding it from all record rows is not.

Do not clone a catalog `FileEntry` for every segment reference. Retain the reference and owner; provide a borrowed or bounded decoded view.

A temporary fused decoder may remove the current duplicate loops as an intermediate commit. It is not the completed solution if it still reconstructs all runtime maps.

### Catalog installation

Give `FileCatalog` an immutable persisted base in addition to its existing constructed base. Opening a snapshot must not treat all restored files as changes relative to an empty repository.

Persist and install the path lookup, file slot state, and version access needed by existing queries and mutation methods. Avoid decoding all paths and then sorting them. Avoid building both an exact-path hash table and an ordered path tree unless distinct operations demonstrably require both; prefer the persisted ordered index when it meets their performance needs.

### Identity and consistency checks

Preserve collection-scoped uniqueness, source revision matching, valid metadata references, slot bounds, and logical/fallback classification rules.

For cross-segment uniqueness, first investigate a streaming merge over existing sorted live-ID iterators, filtering inactive records. This can replace a repository-sized cloned-ID set with bounded merge state. Do not introduce a new generation-wide ID index unless it serves measured query or lifecycle needs as well.

Document the cost of this validation. A merge may reduce allocations while still visiting every live ID; it is not constant-time opening. Avoid replacing one linear validation pass with millions of independently expensive random lookups.

### Writers and maintenance

Create lookup tables during existing sealing or persistence passes. Consume established ordering where available. Sort a previously unordered input once for each genuinely different comparator; reuse that order for encoding, uniqueness checks, and directory construction.

Preserve deferred disk-only work: memory-only readiness must not require building a second persisted catalog or unused lookup encoding. Include all newly deferred work in flush and end-to-end durable-readiness measurements.

Unchanged immutable segments and their lookup blocks must remain reusable across flushes. Measure catalog publication separately; do not turn a small update into an unreported complete segment rewrite.

## 7. Validation and the meaning of direct access

Direct access removes reconstruction. It does not automatically remove all validation or all page touches.

Maintain a validation inventory identifying each invariant, the bytes it reads, when it runs, and whether it requires a full traversal. Distinguish:

- Header, version, configuration, extent, and routing-directory checks.
- Block-local checksum and structural checks.
- Cross-block order and uniqueness.
- Cross-table references, membership completeness, and domain consistency.
- Cross-segment live identity and revision consistency.

Fuse compatible checks into one traversal. Do not decode an owned object merely to validate it and then decode it again for installation.

Where existing checks can safely run on first block access, retain once-only validation state tied to the immutable backing object. Validate routing information before it can establish that a key is absent. Corruption must return an error, never an ordinary no-match result.

If block checksums replace a whole-section checksum, bind the block directory and checksum descriptors to the committed generation. Do not simply omit the whole-section check. A checksum is not a proof of ordering, coverage, or agreement between independently stored tables.

This task must not weaken the existing external-input guarantees merely to reach a startup target. If a global invariant still requires a full scan, retain one allocation-efficient validation pass and report it as remaining startup work. A serialized “validated” flag is not sufficient evidence. Do not move necessary checks behind publication while allowing queries to return incorrect results.

The required endpoint is direct persisted lookup with no derived-table reconstruction and no redundant passes. Fully lazy validation of all metadata is a separate claim and must only be made when the implementation actually provides it.

## 8. Updates, modes, and compatibility

Use immutable persisted bases plus the existing bounded mutation structures. New records, deletions, moves, and changed file versions must override the appropriate base entries. Ordered scans must merge base and changes with correct replacement and tombstone behavior.

A move must not require rebuilding all path entries. Updating one file must not deserialize the full catalog. The first mutation after reopening must not trigger hidden repository-wide reconstruction.

Preserve atomic publication, cancellation, observable storage errors, and pinned readers. New generations must retain the historical metadata references required by their segments. Reclaim old files only through the existing safe retirement mechanism.

Preserve all configured modes:

- Memory-only build/use, default.
- System temporary-directory persistence.
- Repository `.objekte` persistence.
- Advanced custom-directory persistence.
- On-demand or fully resident operation for persisted indexes.

Increment the applicable format version. Reject deprecated index formats through the existing managed-store policy, remove positively identified obsolete index artifacts, and rebuild when persistence is requested. Do not add a production compatibility loader. Never delete unrelated files from a custom directory.

## 9. Verification

Use semantic tests, not tests that merely repeat the new encoding implementation.

Required differential coverage:

- Exact ID/file/path lookup, missing keys, inclusive/exclusive path ranges, and cross-block iteration.
- Empty, single-entry, and multi-block tables; large identifiers; retired file IDs; sparse segment file IDs.
- Multiple records per file, noncontiguous memberships, fallback records, and mixed logical/fallback domains.
- ASCII and Unicode, normal and skip grams, field-specific and custom text bindings.
- Every inserted gram passes the filter; absent grams fall through correctly; swapped filters and malformed parameters are rejected.
- File add/move/delete/update after reopen, repeated flush/reopen, and compaction.
- Old pinned readers retain their old paths, revisions, memberships, and result sets.
- Resident and on-demand readers produce the same results as constructed indexes.
- Truncated blocks, offset overflow, invalid references, duplicate identities/paths, bad domains, wrong generation/binding, and checksum failures.
- Source discovery or source-content scanning does not occur merely to open a valid snapshot.

Test concurrency where validation or cache state is shared. Use immutable reads and worker-local scratch wherever possible; do not introduce a global lock around metadata lookup.

## 10. Measurement and acceptance

Record an immediate pre-change integration-head baseline and the final candidate under identical conditions. Keep the original master baseline as historical context. Do not compare a newer candidate to an old number without identifying the different commits.

Archived reviewed checkpoint: warm on-demand opening 386.26 ms and resident readiness 443.28 ms. These are reference measurements, not a prediction or a target for this change.

### Required instrumentation

Measure separately:

- File opening/mapping or resident reading.
- Root/directory installation.
- Checksums and dictionary validation.
- Catalog/record validation, including cross-segment checks.
- Lookup reconstruction and gram-filter construction.
- Remaining sort/relation/structural metadata decoding.
- Time to readiness, first representative query, and repeated-query latency.

Count dictionary entries visited by purpose, records inspected, owned strings/paths decoded, lookup entries reconstructed, startup sorts, allocation count/bytes, and source files visited. Distinguish mapped bytes, bytes actually read or touched, process RSS, and retained decoded state. Instrumented runs explain behavior; use ordinary release builds for acceptance timings.

### Structural acceptance

For a valid current-format snapshot:

- Zero gram-filter reconstruction traversals.
- Zero reconstruction of record-ID/file-ID maps and per-file membership lists.
- Zero catalog path sorting or path-tree reconstruction.
- Zero full-record traversal solely to derive persisted logical/fallback domains.
- No second equivalent validation/reconstruction pass over the same data.
- No delayed full reconstruction on first query or first update.
- Any remaining full validation traversal is identified, justified, and measured.
- Both resident and on-demand modes use the persisted lookup readers.

### Performance acceptance

Use the established seven-trial paired startup/build measurements and full ASCII/Unicode query campaign, with the same fixture, workers, cache budgets, release settings, and result digests. Measure warm and cold opening separately where the environment permits; describe the cache procedure and do not call a warm run cold.

Report first-query latency after opening as well as warm query median/p95. Include ID lookup, narrow/broad path predicates, text queries, and first update after reopen.

The candidate must demonstrate a reproducible startup improvement and preserve query correctness and smooth query performance. Report construction, initial flush, small-update flush, stored size, RSS, and allocations alongside it. Do not claim a startup win achieved only by shifting equivalent work into the first query or mutation.

Explain material regressions and leave the task incomplete when they remain. Do not invent an arbitrary millisecond target or relax the established query checks to declare success.

## 11. Commit sequence and deliverables

1. **Baseline and format contract:** capture the actual starting state, traversal/validation inventory, lookup consumers, and proposed table layout.
2. **Persisted gram filters:** build during dictionary output, install borrowed views, remove filter reconstruction; verify independently.
3. **Segment lookup readers:** persist record/file/ID/membership/domain access; update consumers and remove reconstructed structures.
4. **Persisted catalog base:** install file/path/version readers; integrate mutation overlays and historical metadata.
5. **Validation consolidation:** remove residual duplicate passes, verify cross-table and cross-segment checks, and test malformed files.
6. **Performance verification and report:** full correctness checks and paired benchmarks from the exact final implementation head.

Intermediate commits may retain adapters, but the final report must identify and remove any adapter that violates the structural acceptance criteria.

Deliver the format documentation, implementation, focused tests, raw benchmark samples, and a report under the repository's established `docs/ai/generated/` and benchmark locations. Include the final SHA, table-by-table reconstruction status, remaining startup scans, first-query/update results, and any unmet limits. Keep the task open until its required outcomes are complete.

## 12. Reviewed source references

All links below are pinned to the reviewed source; the agent must verify current equivalents.

- [Record decoding, membership reconstruction, and validation](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/segment_metadata.rs)
- [Bigram filter construction and dictionary opening](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/bigram_dictionary.rs)
- [Ordered dictionary blocks and validation](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/ordered_block.rs)
- [Generation catalog decoding](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/generation_catalog.rs)
- [Catalog restoration and mutation state](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/lifecycle_catalog.rs)
- [Generation restoration and active-record validation](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/lifecycle_storage.rs)
- [Segment backing storage and section verification](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/crates/core/src/segment_format.rs)
- [Archived performance campaign](https://github.com/Simon-Martens/objekte/blob/e397f8f3046926710e3e75b144d26279e8d4eb83/bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md)

## Implementation report

- **Task:** `direct-snapshot-access`; completed 2026-11-14 on `integration/direct-snapshot-access`, based on `dd3f8feb19df68dd5a5a3ada0d8c99ca91336524`. Integration worktree: `/home/simon/source/objekte-integration-direct-snapshot-access`. The original checkout remained on `master`.
- **Implementation:** persisted and borrowed gram filters; fixed segment record/file slots, memberships, IDs and domains; direct catalog slot/path/version readers with mutation overlays; streaming cross-segment live-ID validation; obsolete-format retirement and typed rebuild. See the three `docs/development/direct_snapshot_*_format.md` contracts and [validation inventory](../generated/direct_snapshot_access_validation_inventory.md). No lookup reconstruction or path sorting remains on current-format managed opens. Section hashes, catalog semantic checks, segment record checks, and cross-segment live-ID validation still run before publication.
- **Branches and commits:** merged `task/direct-snapshot-access/segment` (`966c5bfc39`) and `task/direct-snapshot-access/catalog` (`c0f5293575`) into this integration branch. Integration code through `8747efc47e` is the ordinary release benchmark candidate. `175c94bc15` added feature-gated phase diagnostics; `4d8461cfc9` archived measurements and the report. Earlier format, test, validation, and consumer commits are in this branch's history.
- **Validation:** `CARGO_TARGET_DIR=/home/simon/source/objekte-integration-hybrid-index-storage/target/direct-snapshot-access/integration-build cargo test --locked --workspace` passed on the combined branch after the diagnostic commit. The `detailed-profiling` release example built and ran; ordinary release benchmark examples built without warnings. The full ASCII/Unicode query campaign matched all result digests and every measured persisted open had zero source reads/build parses.
- **Benchmarks:** [full report](../generated/direct_snapshot_access_report.md) and [raw samples](../../../bench/results/direct-snapshot-access/final-20261114/README.md) document commands, fixture, executable hashes, 30-repetition query campaign, seven paired warm opens/builds/flushes/allocations, three paired page-evicted opens and first updates, and phase traces. Warm open medians improved 33–41% across on-demand/resident ASCII/Unicode. First Q167 after open improved or remained near baseline. Gross allocated bytes fell substantially; allocation calls and stored bytes rose.
- **Performance tradeoffs and user direction:** geometric mean query medians regressed 9.2% on-demand and 10.7% resident, although summed per-case median time improved about 5% from broad-scan gains. Initial ASCII flush rose 233.826 to 315.861 ms while snapshot size rose 8.0%; fresh build rose about 2%. These regressions and their raw per-case samples remain visible in the report. The user explicitly asked to finish instead of continuing millisecond-level tuning, so this completion records those deviations rather than claiming an across-the-board speedup. Exact per-purpose dictionary and transient string-decode counters were not gathered; the structural validation inventory identifies the remaining traversals.
- **Cleanup:** after this document is committed and pushed, the original untracked task source will be removed only if its SHA-256 still matches the preserved source prefix. The two merged subordinate worktrees will be checked and removed; the integration worktree and branch remain for review. Temporary task memory will be removed after successful push.

