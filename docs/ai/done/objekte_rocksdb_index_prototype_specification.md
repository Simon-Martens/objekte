# Objekte RocksDB index prototype — implementation specification

Date: 15 November 2026  
Task identifier: `rocksdb-index-prototype`  
Integration branch: `integration/rocksdb-index-prototype`  
Expected integration worktree: `/home/simon/source/objekte-integration-rocksdb-index-prototype`  
Inspected master: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Read-only donor: `abandoned/redb-primary-index` at `09445853ccbce1cdc690a6f1c62bfe765a72e774`

## 1. Objective and completion boundary

Implement a small, isolated RocksDB storage prototype that answers a concrete question: can the Rust RocksDB bindings construct and query Objekte's actual index facts fast enough, with acceptable memory, to justify a later production migration?

Start from master. Reapply selected storage-independent work from the abandoned redb branch. Do not merge that branch, restore the redb migration, replace the production index, or begin a CLI/desktop migration.

The user has selected RocksDB and its Rust bindings. Adding the specified optional dependency, building this prototype, and making the implementation choices fixed below are authorized when this specification is assigned for implementation. Do not ask the user to choose a transaction API, column-family layout, compression format, or benchmark configuration again.

The result is an integration branch containing:

- An optional RocksDB store and direct source builder.
- One-writer and concurrent-writer construction using the same data representation.
- Snapshot-bound storage query primitives.
- A small source-free mutation and persistence correctness harness.
- Independent semantic validation and matched construction/query/memory measurements.
- A documented recommendation to proceed, reject, or investigate one identified limitation.

A reproducible negative performance result is a valid completed experiment. It is not permission to weaken the acceptance targets or to proceed with migration. Do not spend weeks completing production integration before measuring the central hypothesis.

This specification replaces redb-specific assumptions only for this new prototype. It does not modify the abandoned task's disposition.

## 2. Abandoned work and repository discipline

### 2.1 Donor status

The user has abandoned and cleaned up the redb migration:

- The retained remote branch is `abandoned/redb-primary-index`.
- Its retained revision is `09445853ccbce1cdc690a6f1c62bfe765a72e774`.
- The old remote `integration/redb-primary-index` branch was deleted.
- The former redb integration worktree was removed.
- The user reports the task's disposition in `docs/ai/blocked/objekte_redb_primary_index_implementation_plan.md`.

Use the pinned commit as the authoritative donor source. The blocked document was not retrievable at that path in the pinned remote commit during preparation of this specification; it may be local or recorded elsewhere. Preserve the user's local document and comments. Its absence from the pinned commit is not a reason to recreate the abandoned worktree or block this prototype.

Do not rename, delete, update, merge, or push the abandoned branch. Do not depend on its former worktree. Use `git show DONOR:path` or equivalent read-only extraction.

### 2.2 Branch and worktree procedure

Read `AGENTS.md` and `docs/ai/README.md` before implementation. Follow their task-document, explicit staging, commit, push, and completion rules.

1. Record the original checkout path, branch, status, staged changes, and task-document state. Preserve all of them.
2. Fetch master and the abandoned donor from the repository's configured remote. Do not assume the deleted integration branch still exists.
3. Freeze the fetched master commit as the prototype base. If master has advanced from the inspected commit above, record the new base and adapt only the affected interfaces.
4. Verify the donor commit is available and matches the revision above.
5. Create `integration/rocksdb-index-prototype` from the frozen master and create its sibling worktree.
6. Leave the original checkout on master. All implementation, tests, integration commits, and generated results belong to task worktrees.
7. Create a separate, detached sibling baseline worktree at the frozen master if a suitable pristine baseline worktree does not already exist.
8. If the intended branch/worktree already exists, inspect and resume matching task work. Do not reset or overwrite it.

Illustrative commands, with paths and the configured remote verified first:

```bash
OBJ_REPOSITORY=/home/simon/source/objekte
OBJ_REMOTE=origin
OBJ_WORKTREE=/home/simon/source/objekte-integration-rocksdb-index-prototype
OBJ_BASELINE_WORKTREE=/home/simon/source/objekte-baseline-rocksdb-index-prototype
OBJ_DONOR=09445853ccbce1cdc690a6f1c62bfe765a72e774

git -C "$OBJ_REPOSITORY" status --short
git -C "$OBJ_REPOSITORY" fetch "$OBJ_REMOTE" master abandoned/redb-primary-index
OBJ_BASE=$(git -C "$OBJ_REPOSITORY" rev-parse "$OBJ_REMOTE/master")
git -C "$OBJ_REPOSITORY" cat-file -e "$OBJ_DONOR^{commit}"
git -C "$OBJ_REPOSITORY" worktree add -b integration/rocksdb-index-prototype "$OBJ_WORKTREE" "$OBJ_BASE"
git -C "$OBJ_REPOSITORY" worktree add --detach "$OBJ_BASELINE_WORKTREE" "$OBJ_BASE"
```

These are implementation instructions, not a request to execute a migration while merely reviewing this document.

Use one supervising implementer initially. Subagents are optional only after common types and namespace definitions are committed; useful independent assignments are native reference export and fixture validation. Give each subordinate its own task branch/worktree and exclusive file ownership. The supervisor alone edits the integration worktree.

Push immediately after every commit. Never commit to or merge into master. Keep the integration worktree for the user's review.

## 3. Evidence that should guide the prototype

The donor's final A4 report records:

- 148,486 files and records in the measured basic ASCII Musenalm configuration.
- Native construction: approximately 0.991 seconds.
- Final redb construction: approximately 26.478 seconds median.
- 584 publication transactions.
- Approximately 18.6–19.0 seconds in admission.
- Approximately 1.287 GB post-build RSS and 1.300 GB peak RSS.
- A successful aggregate/invariant validation, but no production query/frontend cutover.

These are historical observations, not measurements of RocksDB. The report does not prove that every admission cost was internal to redb, or that copied application code is already optimal.

Preserve the important lessons:

- Preparation and storage admission must be measured separately.
- All required finalization belongs inside the construction interval.
- A bounded input channel is not a total-memory bound.
- Global counters and allocator rows can serialize otherwise independent writers.
- Full native indexes must not coexist with the candidate during candidate timing.
- Aggregate counts alone do not establish semantic equivalence.

## 4. Fixed scope

### 4.1 Required in this prototype

- Direct construction from source using the native adapters and extraction semantics.
- Basic schema-based Musenalm for both ASCII and Unicode alphabets.
- The existing value, length, structural, domain, coverage, sort, relation, raw-text, fallback-text and configured custom-text facts.
- Metadata lookup by physical path, FileId, collection/slot, and logical record identity.
- Sparse/dense bitmap blocks and source-free reverse contributions.
- Whole-physical-file atomic admission, including all selected collection memberships.
- Multiple storage writers against one database.
- Consistent retained snapshots for the implemented primitives.
- Memory-backed operation through RocksDB's memory environment.
- File-backed operation with close/reopen and an explicit durable boundary.
- Focused update, delete and path-move operations after construction.
- Tests for empty, malformed, unreadable, overlapping-collection and multi-record files.
- Independent full-fact comparison on fixtures and the measured corpus.

### 4.2 Explicitly deferred

- Replacing the production `Repository` or `Index` storage.
- Full query-planner/DSL integration, streaming API integration, CLI or desktop switching.
- Complete queries combining indexed and pending source domains during construction.
- Auto-index inference and backfill. Reject an enabled inference option explicitly; do not silently ignore it.
- Concurrent lifecycle mutations while a source build is running.
- Crash-resuming an incomplete build; an incomplete prototype database must be rejected or rebuilt.
- Slot reuse, generation reset in the same database, and online schema changes.
- Memory-environment-to-filesystem export. File-backed construction/reopen is implemented directly; it is not evidence that exporting an already-built memory database is complete.
- External SST ingestion, custom filesystems, engine forks, merge operators, custom comparators, key/value separation, or a general storage-backend abstraction.
- A new retained native index or decoded whole-index cache.

The prototype may expose consistent snapshots of already admitted facts during construction. It must label them as incomplete and must not present that as full progressive query support.

RocksDB is an LSM engine, not a B+ tree. The ordered-key interface preserves the required lookup/range operations, but construction introduces memtable flush and compaction costs. The experiment must include those observations; parallel write support alone is not a performance conclusion.

## 5. Reuse map

All donor paths below are relative to `crates/core/src/index_store/` unless stated otherwise.

| Donor work | Decision | Required adaptation |
| --- | --- | --- |
| `key_codec.rs`, `keys.rs` and codec tests | Reuse | Preserve ordered encoding; prepend the new namespace byte |
| `bitmap_block.rs` and tests | Reuse | Preserve 4,096-slot blocks, sparse/dense boundaries and corruption checks |
| `value_codec.rs` | Reuse selected types/serialization | Replace redb/prototype metadata and publication fields as specified below |
| `contribution_codec.rs` and tests | Reuse | Enable the selected LZ4 envelope policy; add reusable scratch |
| `contributions.rs` | Reuse pure normalization/diff logic | Remove storage-specific coupling |
| `store_build_prepare.rs` | Reuse flat fact types | Move into the prototype module; fix capacity accounting |
| `build.rs::DirectStoreBatchBuilder` and `process_store_batch_profiled` | Reapply selected A4 additions | Feature-gate them; adapt output types; retain source semantics |
| New gram/fact extraction helpers in donor `bigram.rs` and `posting.rs` | Reapply only needed helpers | Keep existing master behavior unchanged |
| `store_build.rs` | Reuse discovery, remapping and semantic algorithms selectively | Replace coordinator-owned admission and publication completely |
| `fts.rs`, `posting_blocks.rs`, `states.rs` | Reuse block-change aggregation algorithms | Replace redb table access and defer global derived statistics |
| `read.rs`, `sorting.rs`, `relations.rs` | Port needed accessors | Snapshot-bound RocksDB reads and namespace-bounded iterators |
| `validation.rs` and tests | Reuse invariant definitions | Add independent exact-fact comparison; remove redb page inspection |
| `feasibility_input.rs` | Reuse reference-producer concepts | Separate from the engine; remove redb imports/backend and repair framing checks |
| `mutations.rs` and tests | Reuse reverse-contribution logic selectively | Prototype-only serial lifecycle operations; no slot reuse |
| `tables.rs` | Reuse names and semantic constants | Replace `TableDefinition` with a fixed namespace registry |
| `mod.rs`, redb backend, table handles, transaction wrappers | Do not copy | Implement a small native RocksDB wrapper |
| Donor CLI switches and benchmark control flow | Do not copy wholesale | Implement the unambiguous mode interface in section 15 |
| Donor reports and failed experiments | Reference only | Do not bring their history or old task state into the new branch |

The A4 implementation commits `1ca6ac7707`, `ae65e29f2b` and `a61bbe1b8c` are useful provenance. Do not cherry-pick them wholesale: they mix reusable algorithms with redb-specific interfaces.

Two observed donor details require correction:

1. `PreparedStoreBatch.retained_capacity_bytes` primarily counts lengths and omits several retained allocations. Count capacities and nested owned buffers; label any unmeasured allocator overhead.
2. Direct preparation derives a provisional coverage cardinality from facts for local slot zero, and the coordinator combines provisional cardinalities using a maximum. Do not reuse this as a final cardinality algorithm. Recompute exact final directory/coverage metrics from the completed facts and compare with the native definitions.

Record every copied source and donor revision in `docs/ai/generated/rocksdb-index-prototype/reuse-manifest.md`. Record changed semantics, rather than merely saying a file was copied.

## 6. Code organization and dependency

Add `crates/core/src/rocksdb_store/` with modules for:

- `mod.rs`: store configuration, owner, errors and snapshot view.
- `namespace.rs`, `key_codec.rs`, `keys.rs`, `value_codec.rs`.
- `bitmap_block.rs`, `contribution_codec.rs`, `contributions.rs`.
- `prepare.rs`, `build.rs`, `write.rs`, `finalize.rs`.
- `read.rs`, `validation.rs`, `mutations.rs`, `reference.rs`.

Do not create dozens of tiny forwarding modules merely to reproduce the donor structure. Preserve adjacent test files where useful.

Expose the module only behind `rocksdb-prototype`. Do not make that a default feature.

```toml
[features]
default = []
detailed-profiling = []
rocksdb-prototype = ["dep:rocksdb", "dep:lz4_flex"]

[dependencies]
rocksdb = { version = "=0.25.0", optional = true, default-features = false, features = ["lz4", "bindgen-runtime"] }
lz4_flex = { version = "=0.14.0", optional = true, default-features = false, features = ["std", "safe-encode", "safe-decode", "checked-decode"] }

[[example]]
name = "bench_rocksdb_index_store"
required-features = ["rocksdb-prototype"]
```

Merge these entries into the existing manifest; do not replace unrelated features or dependencies. Commit Cargo.lock. The inspected binding workspace associates this release series with RocksDB 11.13.2; record the native version actually resolved by the locked dependency rather than assuming a system library version.

Build the bundled native library. Do not set `ROCKSDB_LIB_DIR` or substitute an arbitrary installed library. Use the repository's configured Rust toolchain; the inspected workspace declares Rust 1.99. C/C++ compiler, libclang and build tools are required. Report missing tools directly; do not change the project toolchain to make the dependency compile.

Do not enable unrelated compression backends or optional allocator changes.

## 7. Store and resource configuration

### 7.1 Database type

Use pessimistic `rocksdb::TransactionDB<rocksdb::SingleThreaded>`, shared through `Arc`, with the normal WRITE_COMMITTED policy.

`SingleThreaded` here describes column-family administration, not whether database operations can run on multiple threads. The database is Send/Sync. Do not put a global mutex around it.

Choose this API because the index performs read-modify-write on shared bitmap blocks. Locks avoid repeatedly discarding a large transaction after optimistic conflict detection. Do not implement a second transaction engine in this prototype.

Use one default column family. All logical tables are key namespaces within it. This avoids a separate memtable, file set and configuration per logical table and makes atomic publication straightforward.

### 7.2 Fixed options

| Option | Value |
| --- | --- |
| Comparator | Default bytewise comparator |
| Memtable | Default skiplist |
| Concurrent memtable writes | Enabled |
| Pipelined writes | Disabled in the control configuration |
| Unordered writes | Disabled |
| In-place updates | Disabled |
| Compression | LZ4 |
| Block size | 4 KiB |
| Block cache | 64 MiB |
| Cache index/filter blocks | Enabled |
| Bloom filter | 10 bits/key, whole-key filtering |
| Prefix extractor | None |
| Write buffer size | 32 MiB |
| Maximum write buffers | 3 |
| Minimum write buffers to merge | 1 |
| L0 compaction trigger / slowdown / stop | 4 / 20 / 36 |
| Background jobs | min(2, available logical CPUs), at least 1 |
| Open file limit | 256 |
| Transaction lock-table stripes | 64 |
| Lock timeout | 1,000 ms |
| Deadlock detection | Enabled |
| Transaction snapshot at start | Disabled for fresh admission; use locked current values |
| Two-phase prepare | Disabled/default skip-prepare behavior |
| Custom merge operator | None |

Use the corresponding public methods, including `set_allow_concurrent_memtable_write`, `set_unordered_write`, `set_write_buffer_size`, `set_max_write_buffer_number`, `set_num_stripes` and `set_lock_timeout`.

If a listed binding method or combination is unavailable in the pinned release, do not invent an equivalent unsafe FFI call. Keep the default that preserves correctness, record the unavailable optimization, and continue the supported prototype.

No allocator trimming, automatic full compaction, huge cache, unbounded write-buffer configuration, or durability change may be used to manufacture a passing primary measurement.

### 7.3 Storage modes

**Memory mode — primary acceptance mode**

- Create a fresh `Env::mem_env()` for each store and install it in Options before opening the database.
- Disable WAL through WriteOptions; sync=false.
- Keep environment/cache/database resources alive for the database's entire lifetime.
- All database file contents must remain in the memory environment.
- The Rust binding currently calls host `create_dir_all` before opening even with a memory environment. Use a task-owned empty directory; permit that empty-directory side effect and verify that no database payload, WAL, MANIFEST, SST or LOG is created there. Do not substitute tmpfs and call it the same backend.
- Application result files are separate benchmark output, not database storage.

**File mode — required persistence verification**

- Use the default environment and a fresh, explicitly supplied task-owned directory.
- WAL enabled, sync=false per admission transaction.
- After final completion, call `flush_wal(true)` and report its duration separately as the durable boundary.
- Memtable flushing with wait=true is a further, separately timed observation.
- Never open an existing user database for a destructive build. Refuse a nonempty build destination; reopen is a separate mode.
- Keep the same logical schema, compression and transaction logic as memory mode.

The memory environment still stores an LSM representation, including virtual files, and is not a bound on process RSS. Do not subtract its storage or cache from reported memory.

## 8. Physical schema and encodings

### 8.1 Namespace encoding

A physical key is:

```text
one namespace byte || existing ordered logical key bytes
```

Use this fixed registry:

| Hex tag | Logical namespace |
| --- | --- |
| 01 | store_metadata |
| 02 | collections |
| 03 | fields |
| 04 | bindings |
| 05 | files |
| 06 | file_build_state |
| 07 | pending_files |
| 08 | paths |
| 09 | query_paths |
| 0A | records |
| 0B | logical_ids |
| 0C | file_records |
| 0D | slot_incarnations |
| 0E | free_slots — reserved, empty in this prototype |
| 0F | record_contributions |
| 10 | value_blocks |
| 11 | length_blocks |
| 12 | value_members — reserved, empty |
| 13 | length_members — reserved, empty |
| 14 | state_blocks |
| 15 | state_statistics |
| 16 | value_directory |
| 17 | length_directory |
| 18 | field_coverage |
| 19 | coverage_members |
| 1A | domain_members |
| 1B | sort_definitions |
| 1C | sort_entries |
| 1D | relation_outgoing |
| 1E | relation_incoming |
| 1F | relation_states |
| 20 | fts_blocks |
| 21 | fts_gram_statistics |
| 22 | record_grams |
| 40 | build_receipts |
| 41 | field_coverage_deltas |

Tags are format constants, not enum declaration positions. Reject unknown tags during full validation. Do not represent each namespace as a column family.

Retain the donor's logical component ordering. Integers in ordered keys remain big-endian; signed and exact numeric keys retain their tested order-preserving encodings. Do not serialize ordered keys with ordinary bincode or replace exact numbers with f64.

Prefix ranges must use `prefix_successor` and enforce both namespace and prefix bounds. For all-FF prefixes, retain the explicit namespace/end check. Do not use an arbitrary appended FF byte as a universal upper bound.

### 8.2 Values and headers

Give the prototype its own format identity:

- Magic: `OBJEKTE_ROCKSDB_PROTOTYPE`.
- Prototype format version: 1.
- Explicit scalar, bitmap and contribution codec versions.
- ICU/collation version, configuration digest, alphabet, source root and build identity.
- Storage mode and completed/incomplete state.

Reject mismatches on reopen. Never interpret a redb database or an older experimental RocksDB directory as this format.

Retain the selected A2/A4 payload design:

- Value, length, state and FTS postings use 4,096-slot bitmap blocks.
- Local offsets 0–4,095; sparse payload at cardinality 1–255; dense 512-byte bitset at 256–4,096.
- Delete an empty block instead of storing a zero-cardinality value.
- Store each record's reverse contribution once.
- Store sorted, unique reverse gram lists separately; do not duplicate grams inside the contribution payload.
- Keep the existing LZ4 envelope policy: consider compression at 256 bytes and require at least max(32 bytes, ceil(raw/8)) saving.
- Reuse per-worker serialization/compression scratch. Preserve codec bounds and checked decoding.
- Store sort keys and relation occurrences with the native tie ordering, positions and state semantics.

Do not change block sizes, introduce per-record FTS memberships, or run another compression-format campaign during this prototype.

## 9. Publication, identifiers and avoiding global write contention

### 9.1 Stable scheduling identifiers

Discovery assigns physical FileIds in the same stable order as the native discovery contract. Collection and binding identities must be deterministic for a fixed input/configuration.

The coordinator processes prepared batch descriptors in stable job order and:

- Assigns dense, monotonic slots independently within each collection.
- Maps local prepared slots to the reserved global ranges.
- Resolves/interns binding and sort-definition identities.
- Assigns a unique batch ID.

These are reservations for unpublished work, not visible records. Do not write the global allocator row for every record or every admission. Persist any newly introduced immutable binding definitions before dispatching dependent admission.

Do not create gaps by allocating a whole 4,096-slot block to every 256-record batch. No slot reuse is implemented. Checked overflow is an error.

A failed batch keeps its assigned IDs for bounded retries. A permanently failed build cannot become Complete.

### 9.2 One snapshot, no application-wide commit mutex

Use native RocksDB snapshots. A prototype view borrows the database:

```rust
pub struct StoreView<'db> {
    // Actual binding snapshot and decoded prototype header.
    // No self-reference, lifetime transmute, or leaked database.
}
```

All reads belonging to one operation use the same snapshot, including metadata, pending state, postings, reverse data and sort entries. Do not mix snapshot reads with current `db.get` calls.

Do not recreate redb's per-commit global revision row. A batch ID is not a commit sequence: batch 10 can finish before batch 9. Never treat the greatest completed batch ID as a visibility watermark.

Disable cross-query verified-result caching in the prototype. Snapshot ownership itself defines consistency. A local view ID may label diagnostics but is not a database revision.

### 9.3 Build gate and receipts

Store a `build_gate` control row in store_metadata with build identity, epoch and phase.

Every admission transaction obtains a **shared** `get_for_update(..., exclusive=false)` lock on this row and verifies the captured build/epoch and Building state. Other admissions may hold the same shared lock concurrently. Phase transitions take an exclusive lock.

Never upgrade this shared lock in an admission transaction. An exclusive global gate lock for every writer would defeat the design.

Each successful admission atomically writes one unique build receipt:

- Build identity and batch ID.
- Digest identifying the assigned files, memberships and expected versions.
- Reserved slot ranges for each collection.
- Counts of records and file-membership outcomes.
- Any diagnostic counters required for progress.

A receipt is committed with the data, never before it. Lock/check this batch's receipt immediately after the shared gate and before checking pending-file state. An exact duplicate returns AlreadyAdmitted after aborting the empty attempt; its original pending entries have already been removed. A mismatching reuse of a batch ID is an error.

Read-time progress is calculated from receipts within the same snapshot. Reserved slots are not live until record rows exist in that snapshot.

### 9.4 Derived metadata during construction

Do not update shared value directories, per-gram totals, collection live counts, field cardinalities, or a global publication revision inside every admission transaction.

Instead:

- Bitmap block headers carry the actual block cardinalities.
- Receipts carry additive build progress.
- Per-batch field/coverage facts are stored in field_coverage_deltas under keys including batch ID.
- Global directories/statistics and finalized collection metadata are produced by a bounded finalization pass.
- During Building, accessors requiring finalized statistics return an explicit NotFinalized result, or use a separately named exact scan accessor. They must never return provisional numbers as exact.

This is an intentional prototype scope decision: it preserves atomic facts and snapshots but postpones production planner readiness during construction. It is not evidence of complete progressive-query parity. Finalization cost is fully timed.

## 10. Direct construction pipeline

### 10.1 Preparation

Reuse the A4 flat fact projection. Do not construct a complete native Repository, native posting maps or native gram indexes merely to transpose them into RocksDB.

Small schema descriptors and one bounded batch of extracted facts are permitted. Existing extraction helpers can use temporary native data structures within that batch where they are part of the source contract; they must not become a second authoritative index.

Keep:

- File fingerprints and source revisions.
- Record byte ranges, logical IDs, classes and diagnostic references.
- Per-record normalized values, lengths and structural masks.
- Sorted unique normal/skip grams for each text binding.
- Sort projections, relation occurrences and coverage facts.

Repair diagnostic/slot remapping exactly as in the corrected donor path. Do not infer a file version from a local record offset.

### 10.2 Whole-file work units

A work unit contains a physical file and **all** of its selected collection memberships. Prepare each required collection projection before admitting the file.

The reused donor projector accepts collection-specific groups. It is acceptable to call it for the membership groups within a bounded physical-file batch and combine their flat results. Count any repeated source reads. Do not silently publish one collection's portion and label the entire physical file Indexed.

Batch limits:

- The first work unit is one complete physical file and is submitted promptly.
- Subsequent target: at most 256 physical files, 2,048 total records and 8 MiB source bytes, at whole-file boundaries.
- Do not wait more than a 25-ms coalescing target once complete work is ready merely to fill a batch. This is not a promise to interrupt parsing or a transaction after 25 ms.
- A single file exceeding a target is isolated and committed atomically. Report its size and duration.
- Split a prepared group at whole-file boundaries when the record threshold becomes known.
- No whole-corpus transaction as the primary construction path.

### 10.3 Thread and memory limits

Use separate preparation and admission pools; do not block all Rayon preparation threads on a writer queue.

Let C be available logical CPUs:

- Background jobs B = min(2, C), at least 1.
- Default writers W = min(4, max(1, C/4)).
- Default preparation workers P = max(1, C-W-B).
- Explicit command-line worker counts override these values and are recorded.
- For the writer-scaling comparison, hold P fixed across writer counts.

Use scoped worker lifetimes or owned Arc handles. All workers must terminate before the builder returns.

Preparation budget: 64 MiB soft retained-capacity target. Maximum dispatched but unreleased jobs: min(16, 2P+W), at least one. Account for ready batches **and** prepared inputs retained by writer tasks/retries.

Measure actual Vec/String capacities, nested buffers, encoded key/value buffers, block accumulators and transaction-input ownership. Running parsers can exceed an estimate; report their memory separately and stop further dispatch when the retained target is exceeded. Known oversized files run in isolation. Do not describe the 64-MiB target as a hard total-RSS cap.

Do not free a job's budget reservation when it is merely sent to a writer. Release it after commit/abort and buffer release.

Dense adjacent batches can share the same 4,096-slot block, especially for common grams and structural facts. Multiple active transactions therefore do not prove useful parallel admission. Record each batch's collection/block ranges, overlap with concurrently active batches, and time waiting for conflicting keys. Keep a disjoint-block fixture as the control that demonstrates the engine path can progress concurrently. If the bounded preparation window cannot expose disjoint ranges on the real corpus, report a scheduling/layout limitation. Do not allocate sparse slot ranges, enlarge memory budgets, or claim that RocksDB itself lacks concurrent writes to conceal that result.

### 10.4 Admission transaction

Build a bounded mutation plan before opening the transaction:

- Blind inserts/deletes for independent rows.
- Grouped bitmap additions/removals by full physical key.
- Required file-version checks.
- Receipt and per-batch coverage rows.

For a fresh build, updates are primarily additions. Still aggregate duplicate membership facts before encoding.

Then:

1. Open a transaction with the prescribed WriteOptions/TransactionOptions.
2. Acquire the shared build-gate lock and validate build/epoch/state.
3. Exclusively lock/check the one receipt key for this batch. Return AlreadyAdmitted for an exact existing receipt, or fail for a mismatch. Then process all remaining lock-bearing keys in ascending physical byte order. Include blind writes in this order; otherwise their implicit locks can reintroduce deadlock cycles.
4. Read file states and mutable bitmap blocks through `get_for_update`/`get_pinned_for_update` with exclusive=true where a write depends on the old value.
5. Validate the exact file version and required pending memberships.
6. Decode each changed block once, apply its deduplicated changes, encode once, and put/delete once.
7. Write records, identity/path mappings, reverse contributions, reverse grams, sort/relation rows, per-file state, pending removals, coverage deltas and receipt in the same transaction.
8. Commit. Publish progress notification only afterward.

The universal admission lock order is gate, this batch's single receipt, then all other keys in byte order. Every admission uses that same order; no admission locks another batch's receipt. Steps 3–7 then describe one ordered traversal of the remaining mutation plan, not separate unordered write loops. Each unique physical key has its validation/read/modify/write action performed when reached; any later validation failure rolls back the entire transaction. Writing the already-locked receipt at the end adds no new lock. Preserve legitimate occurrence multiplicity where the schema includes occurrence positions.

Do not perform a read outside the transaction followed by an unprotected write of the resulting bitmap. Two writers setting different bits in one block must preserve both.

Do not use Merge, `WriteBatch` without conflict protection for shared blocks, or a coordinator mutex spanning admission. One writer and W writers use exactly the same transaction function.

Transaction callback side effects must be repeatable. Retry only documented contention errors (Busy/TimedOut as appropriate), at most three retries after the initial attempt, with 1/2/4-ms backoff. Reuse captured prepared facts and IDs; do not reparse source on a lock retry. Count retries and wait time. Other storage errors fail the build, preserve evidence, and are not retried blindly.

Cancellation stops dispatch, drains/joins active workers, and marks the build Cancelled through the exclusive gate. Incomplete state remains identifiable. Crash resume is deferred.

### 10.5 Finalization

After all expected batches committed and writer workers joined:

1. Exclusively transition the gate to Finalizing.
2. Scan completed records/receipts to calculate exact collection counters and high-water marks.
3. Stream each block namespace in order to construct value/length directories, structural statistics and gram statistics.
4. Merge coverage deltas using native semantic rules. Compute distinct-value/length cardinalities from the completed index, not sums or maxima of batch-local distinct counts.
5. Check required pending/outcome accounting. Unreadable files have explicit Unavailable outcomes; they are not falsely marked indexed.
6. Write derived metadata in bounded batches (maximum 8 MiB encoded writes per finalization batch, except one oversized value).
7. Publish Complete in a final transaction only after all required metadata exists.
8. Release finalization snapshots, preparation buffers, worker pools and scratch. Keep the database open.

The outer readiness timer ends after step 8. Include pool creation, database opening, discovery, preparation, admission, finalization and release of construction-owned buffers. Do not move these costs into the first query.

## 11. Read primitives and allocation rules

Port these operations against a single StoreView:

- Record by collection/slot and logical identity.
- File by ID, native path and query path; records belonging to a file.
- Exact value members; value range members.
- Length range members.
- Structural-state enumeration, cardinality and point membership.
- Coverage/domain membership.
- First sort page, forward/reverse, with native tie ordering.
- FTS block enumeration and candidate intersection.
- Reverse contributions and reverse grams.
- Outgoing/incoming relation occurrences.

Use raw iterators and borrowed slices in tight loops. Check iterator status after traversal. Use pinned gets when their lifetime avoids a value allocation. Decode into reusable scratch. Do not collect an entire keyspace when a prefix/range traversal suffices.

Each iterator needs explicit lower/upper bounds and a namespace check. No prefix extractor is configured; use total-order semantics. Reject malformed key/value encodings rather than skipping them.

For final query-primitive benchmarks, use one snapshot and the same complete result consumption on native and RocksDB sides. Compare actual sorted IDs/counts or digests, not iterator construction time. Candidate generation is not final FTS query execution: label it accordingly.

Do not wire these accessors into the production query planner in this task. Record the full query-integration work that would remain.

## 12. Small lifecycle and persistence harness

After Complete, allow a **serial lifecycle mutation harness**. Concurrent initial construction remains the performance target; this harness establishes semantic viability without implementing the full lifecycle controller.

Required operations:

- Add a complete file from prepared facts.
- Replace a file atomically by removing its old contributions and adding the replacement.
- Delete a file using stored reverse contributions, without reading its source.
- Move its path while retaining identity where the native contract does so.

Use one transaction for each physical-file mutation across all affected namespaces/collections. Validate file version. Allocate new slots monotonically; do not reuse deleted slots. Existing donor free-slot algorithms are not required.

Maintain affected derived statistics exactly within these serial mutations. No full database rebuild for a single update. If mutation planning reads old reverse facts before opening a transaction, validate their file version under the transaction before applying the plan.

Keep an old snapshot while performing each operation. Assert that it continues to return the complete old state, while a new snapshot returns the complete new state.

File mode verification:

- Build a small fixture, reach Complete, establish the durable WAL boundary, close and reopen.
- Compare full facts, not only record counts.
- In a child process, establish a durable boundary and signal the parent; terminate the child without graceful database drop, reopen and validate the durable state.
- Reject or explicitly identify a database left in Building/Finalizing.
- Do not claim durability for writes after the last explicitly acknowledged durable boundary.

Memory mode verification:

- Build/flush/query/drop through a memory environment.
- Verify no database files appeared in its host directory.
- A new memory environment must not recover the prior store.
- Do not call a disk-only reopen result a test of memory-to-disk export.

## 13. Correctness validation

### 13.1 Independent native oracle

Run native reference construction in a different process from candidate performance measurement.

Reuse native reference export ideas from `feasibility_input.rs`, but do not use the candidate's direct projection as the only expected-result generator. Export canonical semantic facts from the native Repository.

Normalize only internal storage identifiers:

- Map collection to its name.
- Map records to logical identity plus source locator where necessary.
- Map binding IDs to complete binding identities.
- Ignore prototype instance IDs, slot numbers, batch IDs and timing fields.
- Preserve paths, source ranges, statuses, diagnostics, numeric values, presence/null distinctions, gram kinds, text binding identity, relation positions and sort ties.

Compare every fact family. Counts are supporting diagnostics. Use deterministic per-family streams and cryptographic digests; retain the first mismatching keys/facts. For small fixtures, compare full expanded facts directly.

Reference streams must reject truncated frames, missing End markers, mismatching manifests, duplicate keys in namespaces requiring uniqueness and oversized messages. Preserve legitimate repeated relation occurrences and duplicate source logical IDs according to native semantics; include source locators/occurrence positions in their canonical identity rather than collapsing them. The donor's reader treats UnexpectedEof while reading the four-byte length as ordinary end-of-stream; fix it so partial length headers are errors. Keep bounded messages and do not deserialize an entire corpus into one buffer.

The initial corpus manifests must match source/configuration digests, collection definitions, schema files, alphabet and extraction options. No acceptance comparison across mismatching manifests.

### 13.2 Required focused tests

1. Ordered key round trips and comparisons: negative/positive exact decimals, equivalent numeric values, dates, bools, strings/IDs, embedded NULs and non-ASCII.
2. Prefix boundaries, including empty prefix and FF suffixes; no namespace leakage.
3. Bitmap cardinalities 0, 1, 255, 256, 4,096; duplicate changes; offsets 4,095/4,096 across blocks.
4. Corrupt/truncated codecs and contribution envelopes.
5. Record metadata, paths, logical identity and reverse contribution parity.
6. Empty files, malformed fallback, unreadable files and multi-record JSONL.
7. One physical file in overlapping collections is never partially admitted.
8. Two writers updating different bits in the same block; no lost updates.
9. Disjoint-block concurrent writers; the application does not serialize the whole transaction with a mutex.
10. Old/new snapshot atomicity across records, postings, grams, reverse rows and file state.
11. Transaction rollback leaves no receipt or partial file.
12. Stale epoch/file-version rejection and exact duplicate batch handling.
13. Out-of-order preparation and commits; deterministic final semantic facts.
14. Coverage/statistics exactness across multiple batches, including repeated distinct values.
15. Source-free delete and atomic replacement/path move.
16. Retained-reader behavior during mutation and after release.
17. Memory-environment file isolation and file-backed durable reopen.

Concurrency tests should use barriers and bounded timeouts around the relevant operations, not timing-sensitive sleeps as proof of correctness.

An existing aggregate manifest is not a substitute for these checks. Do not report zero validation errors as full semantic parity unless the independent comparisons also ran.

## 14. Timing, memory and performance decision

### 14.1 Required observations

Capture:

- Database-open, discovery, preparation work, coordinator mapping, writer queue wait, lock/RMW/encoding/commit, finalization and total readiness.
- Wall time separately from summed worker time.
- Time of the first complete-file commit and all commit timestamps.
- Requested/resolved preparation writers/background jobs; peak simultaneous admission transactions.
- Batches, physical files, memberships, records, changed blocks, encoded bytes, lock waits/retries.
- Process RSS/HWM before opening, at readiness, after measurement settlement, and during retained-snapshot mutation tests.
- RocksDB properties where available: memtable sizes, block-cache usage/pinned usage, estimated table-reader memory, SST size, running flushes/compactions and pending compaction bytes.
- Real disk footprint in file mode. Virtual SST properties are diagnostics in memory mode, not a full memory-accounting replacement.
- Build/compiler/library versions, CPU affinity, OS, allocator, executable digest and command arguments.

Unavailable engine metrics must be null/unavailable with an explanation, not zero.

Memory inspection must include all native allocations visible to process RSS. Do not subtract the cache, virtual files, or baseline executable memory to pass a gate. Do not compare tmpfs memory against heap-only accounting.

### 14.2 Logical readiness versus maintenance

Primary `ready_ns` includes all required logical finalization. It does not require a forced full compaction.

Measure queries immediately after readiness while ordinary maintenance can still run. Also measure after a separate wait=true memtable flush and bounded maintenance observation. Record the time and background work in each case. Do not hide a compaction backlog, and do not add unnecessary full compaction to one engine's primary build time.

Use a maximum 30-second settlement observation per run; a timeout is an observed result, not permission to wait indefinitely. Resource samples during this interval are diagnostics; the primary readiness RSS remains unchanged.

Close/drop duration is recorded separately. Do not substitute teardown timing for durable-boundary timing.

### 14.3 Matched baseline and gates

Use a fresh native measurement from the frozen master, with the same basic Musenalm definitions and alphabet. The existing `bench_build_musenalm` uses `load_definitions_with_schemas`; `bench_index_construction` defaults to extended definitions. Do not accidentally compare those two configurations.

Primary time boundary is caller-visible construction after definitions are loaded and before validation/query/output. For RocksDB it includes creating/opening the store. Also report internal stage totals separately.

Retain the previous objectives:

- Construction readiness <= 1.15 × matched native caller construction time.
- Post-build RSS <= 1.40 × matched native post-build RSS.
- Peak construction RSS <= 1.60 × matched native peak RSS.
- All semantic/correctness checks pass.

Calculate from raw bytes/nanoseconds and record the rounding rule for integer limits. Use floor for maximum integer-byte allowances. Historical values are context only.

Early query-primitive rejection criterion: no representative median exceeds 2 × native + 100 microseconds without an attributed explanation. This is a prototype screening threshold, not final production query acceptance.

For each primitive, select deterministic absent, selective and common cases from the native reference; include a range crossing a bitmap boundary and sort ties. Record the exact probes in the campaign manifest. Use 10 untimed warmup iterations and 100 measured iterations per probe, with complete result consumption and a correctness digest outside the timed interval. Report median and p95; also retain the first execution separately. Do not compare a fully materialized native result with only a RocksDB iterator setup. The mutation campaign uses the small fixture for exhaustive correctness plus 100 deterministic add/replace/move/delete cycles on disposable copies of the selected corpus, with stage timings and RSS observations. No production repository files may be mutated.

Report both the pristine-master native baseline and the candidate branch's native control. A regression in the native control indicates that the copied hooks changed master behavior or measurement; investigate it before attributing a difference to the engine.

### 14.4 Bounded campaign

1. Run tiny correctness fixtures and the memory/file environment verification first.
2. Run one full ASCII native control and one full ASCII RocksDB construction with W=1.
3. Run W=2 and W=4, with P held fixed. If C permits and W=4 improves admission meaningfully, run W=8.
4. Select the fastest valid configuration by readiness time, subject to memory and correctness. Do not select by commit-only time.
5. Run three alternating fresh-process native/candidate pairs for ASCII; repeat for Unicode.
6. Run representative native/candidate primitives and the mutation/snapshot campaign in separate processes.
7. Run one file-backed construction/reopen campaign with the selected configuration; do not mix file-mode figures into memory-mode acceptance.

If the full build exceeds 5 × native after reproducible runs, or memory exceeds its budget by more than 50%, profile the dominant interval and allow one localized repair within this schema. If it still fails, report a negative prototype. Do not launch an unbounded tuning campaign.

Allowed single optimization experiment after the control:

- Pipelined writes on versus off, only if supported by the selected TransactionDB configuration and all snapshot tests still pass.

Do not add optimistic transactions, alternate block sizes, per-worker databases, SST ingestion, Merge operands, new compressors or enlarged budgets to chase a passing number. A later experiment can be proposed with evidence.

## 15. Benchmark interface and execution

Add `crates/core/examples/bench_rocksdb_index_store.rs`. Reuse existing benchmark support and schema loaders. Implement one mutually exclusive `--mode`:

| Mode | Behavior |
| --- | --- |
| native-build | Build native Repository, measure, release; never open RocksDB |
| source-build | Build only the RocksDB store directly from source |
| reference-export | Build native reference and export canonical facts/manifests |
| validate | Build RocksDB, compare with supplied reference, run invariants; correctness process |
| primitives | Build selected backend, run exact storage-operation probes |
| mutations | Build a small fixture or selected corpus, execute mutation/snapshot harness |
| reopen | Open an existing completed file-mode prototype and validate it |

Arguments:

- `--root PATH`; default `examples/musenalm`.
- `--alphabet ascii|unicode`; default ascii.
- `--schema-profile basic|extended`; default basic; always record it.
- `--storage memory|file`; default memory.
- `--db-path PATH`; required for file mode; refusal rules apply.
- `--prepare-workers auto|N`.
- `--write-workers N`; default as section 10.3.
- `--backend native|rocksdb` only for primitives.
- `--reference PATH` for validate/reopen.
- `--result-dir PATH`; required; create a unique child directory.
- `--profile` requires detailed-profiling and never silently changes ordinary timing.
- `--pipelined-writes` selects the one optional experiment.

Reject incompatible combinations. Inference is unsupported and must be rejected explicitly. Do not reproduce the donor example's contradictory `--mutations`/`--build-source` checks.

Default timing modes never execute full validation or query warmup before readiness memory samples. Validation and primitive results are separate artifacts.

Build once, then invoke the executable directly for fresh-process trials. Do not include Rust/C++ compilation in construction measurements.

Illustrative commands from the appropriate worktrees:

```bash
cargo build --release -p objekte-search-core --example bench_build_musenalm
cargo build --release -p objekte-search-core --features rocksdb-prototype --example bench_rocksdb_index_store

target/release/examples/bench_build_musenalm \
  --root /absolute/path/to/musenalm \
  --alphabet ascii \
  --build-workers auto

target/release/examples/bench_rocksdb_index_store \
  --mode source-build \
  --root /absolute/path/to/musenalm \
  --schema-profile basic \
  --alphabet ascii \
  --storage memory \
  --prepare-workers 8 \
  --write-workers 1 \
  --result-dir bench/results/rocksdb-index-prototype/ascii-w1
```

The example worker count is illustrative; record the actual fixed P for the campaign. Keep baseline and candidate build directories distinct. Capture pristine native output using the supervisor's driver.

Use `bench/run_rocksdb_index_prototype.py` as a small subprocess/campaign driver if automation is needed; reuse the repository's existing benchmark conventions and JSON reporting. Do not create a new generic benchmarking framework.

Required machine-readable outputs:

- `campaign-manifest.json`: revisions, environment, corpus/configuration identities and options.
- Per-run `build.json`, `memory.json`, `primitives.json` as applicable.
- `validation.json` with family-level equality and invariant results.
- `comparison.json` with ratios, limits, missing rows and pass/fail classifications.

A comparison must reject missing samples, mismatched manifests and failed correctness. It must not treat missing/null timings as zero. Performance failure is reported as failure even when the implementation experiment itself is complete.

## 16. Implementation phases and commits

Proceed sequentially until the first representative construction result. Do not spend the initial effort on a generalized abstraction or production API.

### Phase 0 — isolate and freeze

- Read instructions, create worktrees and freeze master/donor.
- Record scope and reuse decisions.
- Verify compiler/native dependency prerequisites.
- Establish the pristine native baseline command and corpus identity.

Suggested commit: `Define RocksDB prototype scope and source provenance`.

### Phase 1 — engine foundation and reusable formats

- Add the optional binding dependency and feature.
- Implement one-CF namespace storage, memory/file modes and borrowed snapshots.
- Port codecs and their meaningful tests.
- Verify transactions, same-block concurrent updates, snapshot retention and memory-file isolation on a tiny fixture.

Suggested commit: `Add optional RocksDB store and snapshot primitives`.

Do not proceed if memory mode writes database payload to the host filesystem, snapshots mix versions, or concurrent updates lose bits.

### Phase 2 — direct build and early feasibility result

- Reapply flat extraction helpers with feature gating.
- Implement bounded preparation, deterministic mapping, shared gate, receipts and concurrent admission.
- Implement timed bounded finalization.
- Add native/source-build/reference/validate modes and exact-fact fixture comparisons.
- Run the first full ASCII W=1/W=2/W=4 construction measurements immediately.

Suggested commit: `Build index facts through bounded concurrent RocksDB admission`.

If the early failure rule triggers, perform the permitted localized repair or stop with a documented rejection. Do not implement deferred migration work.

### Phase 3 — query and lifecycle evidence

If Phase 2 remains viable:

- Complete the listed read primitives and their native equivalents.
- Add source-free mutation, old-snapshot and file-reopen tests.
- Run ASCII/Unicode pairs, primitive probes and memory observations.
- Complete independent semantic comparison for all included fact families.

Suggested commit: `Validate RocksDB reads mutations and persistence`.

### Phase 4 — decision and integration report

- Run required feature-off and feature-on tests.
- Review all changes outside the prototype module.
- Generate the comparison and a clear proceed/reject/limited-evidence recommendation.
- Complete task-document lifecycle and push the report.

Suggested commit: `Report RocksDB prototype feasibility and remaining migration work`.

If the prototype is rejected early, mark later performance/migration work as intentionally not undertaken under the early-exit rule; retain all required tests for implemented code. Do not claim unexecuted validation passed.

## 17. Validation commands and delivery criteria

At minimum:

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p objekte-search-core --all-targets --features rocksdb-prototype
cargo test -p objekte-search-core --features rocksdb-prototype
cargo check -p objekte-search-core --all-targets --features rocksdb-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/rocksdb-prototype
```

Use repository-documented formatting/lint procedures on changed files. Do not reformat unrelated code. If an environment-dependent workspace test cannot run, preserve the exact command/error and report it rather than claiming the full suite passed.

Feature-off builds must not compile/link RocksDB. Native CLI/desktop behavior must remain unchanged. The abandoned redb branch and original checkout must remain untouched.

Store concise reports under:

- `docs/ai/generated/rocksdb-index-prototype/reuse-manifest.md`
- `docs/ai/generated/rocksdb-index-prototype/prototype-report.md`
- `docs/ai/generated/rocksdb-index-prototype/comparison.json`

Keep large raw benchmark outputs under the repository's existing benchmark-results convention. Do not commit entire generated database directories, native build output or copied corpus data.

The final report must answer:

1. Which exact donor work was reused?
2. Does the application execute admission concurrently?
3. How much time is spent waiting on shared bitmap keys versus engine commit/flush?
   Distinguish useful overlap from transactions that merely coexist while waiting; identify whether bounded scheduling exposes independent block ranges.
4. Are all included facts independently equivalent?
5. What are full readiness time, RSS/HWM and primitive-query ratios?
6. How much maintenance continues after readiness?
7. Did memory mode actually avoid host database files?
8. What did durable reopen and retained-snapshot tests establish?
9. Which requirements remain deferred, especially pending-domain queries, inference, slot reuse and memory-to-disk export?
10. Is a production migration justified by the measured result?

Append an Implementation report to the completed task document and follow `docs/ai/README.md`. A negative but correctly measured prototype can be completed and reported; missing required environment/correctness evidence remains explicitly incomplete.

Keep the new integration branch/worktree for the user. Do not merge it into master automatically.

## 18. Source references

Repository evidence:

- [Inspected master](https://github.com/Simon-Martens/objekte/tree/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9)
- [Pinned abandoned donor](https://github.com/Simon-Martens/objekte/tree/09445853ccbce1cdc690a6f1c62bfe765a72e774)
- [Final A4 report](https://github.com/Simon-Martens/objekte/blob/09445853ccbce1cdc690a6f1c62bfe765a72e774/docs/ai/generated/redb-primary-index/construction-a4-report.md)
- [Donor index-store modules](https://github.com/Simon-Martens/objekte/tree/09445853ccbce1cdc690a6f1c62bfe765a72e774/crates/core/src/index_store)
- [Donor direct preparation](https://github.com/Simon-Martens/objekte/blob/09445853ccbce1cdc690a6f1c62bfe765a72e774/crates/core/src/build.rs)
- [Master benchmark](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/crates/core/examples/bench_build_musenalm.rs)
- [Repository task workflow](https://github.com/Simon-Martens/objekte/blob/c54486f893a57fac0b1b80ed967ab4f5a5fd26c9/docs/ai/README.md)

Binding and engine references:

- [Rust RocksDB binding](https://github.com/rust-rocksdb/rust-rocksdb)
- [TransactionDB API](https://docs.rs/rocksdb/latest/rocksdb/struct.TransactionDB.html)
- [Transaction API](https://docs.rs/rocksdb/latest/rocksdb/struct.Transaction.html)
- [TransactionOptions API](https://docs.rs/rocksdb/latest/rocksdb/struct.TransactionOptions.html)
- [Memory environment](https://docs.rs/rocksdb/latest/rocksdb/struct.Env.html)
- [Options API](https://docs.rs/rocksdb/latest/rocksdb/struct.Options.html)
- [RocksDB transaction semantics](https://github.com/meta/rocksdb/wiki/Transactions)
- [Concurrent memtable insertion](https://github.com/meta/rocksdb/wiki/MemTable)
- [RocksDB memory accounting](https://github.com/meta/rocksdb/wiki/Memory-usage-in-RocksDB)
- [Merge-related parallel-write limitation in 11.13.2](https://github.com/meta/rocksdb/blob/v11.13.2/db/db_impl/db_impl_write.cc)
- [Rust wrapper directory creation](https://github.com/rust-rocksdb/rust-rocksdb/blob/master/src/transactions/transaction_db.rs)

This specification is based on source/document inspection. No RocksDB prototype benchmark was executed while writing it.

## Implementation report

**Task identifier and completion:** `rocksdb-index-prototype`, completed
2026-11-15. Base commit
`c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`; integration branch
`integration/rocksdb-index-prototype`; integration worktree
`/home/simon/source/objekte-integration-rocksdb-index-prototype`.

**Implementation commits:** `b7685741fe` defined scope, provenance,
prerequisites and the frozen native baseline. `a103c89b59` added the optional
RocksDB dependency, one-CF pessimistic store, memory/file environments,
borrowed snapshots, namespaces, ordered codecs, bitmap blocks, contribution
compression and foundational tests. `ab797ecf36` added direct source
preparation, bounded whole-file scheduling, concurrent transaction admission,
shared-gate/receipt handling, exact finalization, native-reference fact
digests, invariant validation, benchmark modes and the first full construction
campaign. This report and the final comparison commit follow those commits.
Every implementation commit was pushed immediately.

There were no subordinate branches or worktrees. The original checkout stayed
on `master`; all implementation and evidence were created in the integration
worktree. The task-owned baseline worktree was removed after its matched P=8
output was committed and its clean state was verified. The obsolete Phase 2
stash was dropped after its code was restored, reviewed and committed. The
integration worktree and branch remain for review.

**Implementation and deviations:** The Phase 1 and Phase 2 code implements one
default column family, native snapshots, independent preparation/admission
workers, whole-physical-file atomicity, shared build-gate locking, per-file
receipts, bounded finalization and direct extraction without a retained native
candidate index. A first construction experiment used one transaction per
physical file; the permitted localized repair grouped bounded whole files into
582 transactions, reducing W=1 readiness from 345.661 s to 155.064 s.
Four writers reduced the final run to 76.344 s. W=4 was selected over W=8
because the latter saved less than 5% while increasing RSS.

The prototype is rejected. Its final W=4 readiness is 81.56 times the matched
pristine-native build, resident RSS is 12.09 times native, and peak RSS is 12.49
times native. These fail the 1.15 time, 1.40 resident-memory and 1.60
peak-memory gates by an order of magnitude. Under the specification's
early-exit rule, primitive-query timing, source-free mutation campaigns,
Unicode corpus measurements, file-backed measured-corpus reopen, full-corpus
native reference export and production migration were intentionally not
undertaken. The implemented file/memory primitives retain their focused tests.

The candidate native control initially regressed because an exploratory donor
scheduler multiplied normal preparation weights by eight. That non-prototype
change was removed. The final native control is within 3.34% of pristine time,
1.94% of resident memory and 1.91% of peak memory, so the final engine
comparison is not attributed to a native-control regression.

**Validation:** `cargo test --workspace` passed 820 tests with 17 ignored and
zero failed. `cargo check --workspace --all-targets` passed.
`cargo test -p objekte-search-core --features rocksdb-prototype --lib` passed
547 tests with 15 ignored and zero failed.
`cargo check -p objekte-search-core --all-targets
--features rocksdb-prototype,detailed-profiling` passed. Changed Rust files
pass `rustfmt --check`; `git diff --check` passed. Workspace-wide rustfmt still
reports the inherited repository formatting drift.

The final full ASCII validation has zero errors across 148,486 records,
148,486 file records/receipts, 2,814,411 values, 921,173 lengths, 14,004,908
structural states, 56,275,908 gram memberships and 566,768 sort entries.
Independent native/prototype family digests pass on overlapping typed and
malformed/empty fixtures. Memory-mode construction leaves its host directory
empty.

**Measured evidence:** Matched P=8 pristine native build is 936,001,020 ns with
404,692,992-byte peak RSS. The final RocksDB W=4 run is 76,344,067,646 ns with
5,054,058,496-byte peak RSS and 582 admission transactions. Raw runs are under
`bench/results/rocksdb-index-prototype/`. The comparison is
`docs/ai/generated/rocksdb-index-prototype/comparison.json`; the detailed
rejection report is
`docs/ai/generated/rocksdb-index-prototype/prototype-report.md`; donor reuse
and semantic adaptations are in
`docs/ai/generated/rocksdb-index-prototype/reuse-manifest.md`.

**Remaining limitations:** This experiment rejects the tested RocksDB
construction layout, not the engine universally. Admission/backpressure is the
dominant cost, and common structural/gram blocks create substantial contention.
The repository retains no production planner/CLI/desktop changes, no inference
support, no slot reuse, no crash resume and no memory-to-file export.
