# Agent specification: hybrid store generation directories, persistent validation, and posting-proof reuse

Task: `hybrid-index-storage` (continuation)
Repository: `Simon-Martens/objekte`
Plan date: 14 November 2026
Prepared against branch head: `2ba53f3a2035a7f72f26a3d222553ac3a52cc3a0` on `integration/hybrid-index-storage`

## 1. Objective and scope

Implement exactly three changes on the existing hybrid index storage work:

- **A. Generation directories with grace garbage collection.** Replace the flat managed-store layout with immutable `generations/<n>/` directories. A reader resolves the current generation from one atomically replaced manifest and takes **no lock**. A writer keeps the exclusive `store.lock`, hard-links unchanged segment files into the new generation, publishes the manifest, and deletes generation directories older than a grace window.
- **B. Persistent per-segment validation stamps.** Record that the mutable-open semantic validation has already been performed for exact immutable segment bytes and an exact validator version. Reopens with a matching stamp skip the expensive eager leaf/ordering traversal while keeping all framing, bounds and checksum checks. A stamp is written only after a successful full validation, and is never written for unvalidated bytes.
- **C. Posting-validation proof reuse without a per-dictionary mutex map.** Remove `Mutex<HashMap<PostingDescriptor, Arc<OnceLock<Result<(), String>>>>>` from `SealedPostingDictionary`. A validated posting carries its proof implicitly by entering the decoded-posting cache; small uncached postings are validated on access. Corrupt payloads remain hard errors, never empty postings.

These are repairs to the existing hybrid storage implementation. They do not authorize changing query semantics, the index format's durable invariants, storage policies, or UI behavior beyond status/diagnostic fields. They are not the whole `hybrid-index-storage` task and must not be reported as task completion.

Non-goals:

- Do not redesign postings, dictionaries, bigram structure, sort columns, relation storage, or inference.
- Do not add a new database dependency for locking, caching or hashing.
- Do not add reader reference counting, lease files, or in-generation pin markers. Grace plus retry is the selected contract.
- Do not implement a dense descriptor-ordinal table unless Change C's measurement gate requires it (Section 7.5).
- Do not remove or weaken external-input validation, checksums, slot-domain checks, or the rule that corrupt input produces errors.
- Do not move construction or validation work behind the query timer to make a benchmark pass.
- Do not merge into `master`.

## 2. Base revision, worktree and workflow

Start by recording the actual branch head. The implementation agent must rebase its plan on that recorded commit; the reviewed head was `2ba53f3a2035a7f72f26a3d222553ac3a52cc3a0`.

Continue the existing task; do not create a competing integration branch.

- Integration branch: `integration/hybrid-index-storage`.
- Integration worktree: `/home/simon/source/objekte-integration-hybrid-index-storage` (create only if absent; keep it a sibling of the original checkout).
- Subtask (optional, for Change C only): branch `task/hybrid-index-storage/posting-proof` in `/home/simon/source/objekte-task-hybrid-index-storage-posting-proof`.
- Keep `/home/simon/source/objekte` on `master`.
- Read `AGENTS.md` and `docs/ai/README.md` before implementation.
- Stage explicit paths; push every commit immediately; do not force-push, rewrite published history, or modify remotes.
- Run `python3 scripts/format-rust-spacing.py` after `cargo fmt`, then check with `--check`.
- Preserve unrelated user edits and uncommitted task documents.

Coordination constraint: another agent is actively modifying `lifecycle.rs`, `hybrid_segment_codec.rs`, `posting_dictionary.rs`, `ordered_block.rs`, `segment_format.rs` and `bigram_dictionary.rs` for Repairs 1–2 of the repair specification. Do not run concurrent edits to the same files. Recommended order:

1. Change C (mostly `posting_dictionary.rs`) may run as a subtask only if the supervisor confirms the sealing series has merged into the integration branch.
2. Change B and Change A are a single sequential series owned by one agent, because both alter `managed_store.rs` and the manifest.
3. If in doubt, wait and integrate; do not resolve a conflict by discarding the other agent's proofs.

## 3. Current implementation map

Files and entry points the implementation must read before editing:

| Area | File | Current behavior |
| --- | --- | --- |
| Store root/paths/policy | `crates/core/src/index_storage.rs` | `IndexStoragePolicy`, `IndexResidency`, `ResolvedIndexStorage`; `managed_directory` is the flat store directory |
| Manifest, lock, commit, reclaim | `crates/core/src/managed_store.rs` | flat `manifest.bin`, `store.lock`, `catalog-*.bin`, `segment-*.idx`; writer lock; readers take `lock_shared()` in `read_generation` |
| Lifecycle flush/restore | `crates/core/src/lifecycle_storage.rs` | `flush`, `open_with_storage`, `restore_generation`, `install_committed_sealed_segments`, `mark_storage_opened` |
| Segment envelope/cache | `crates/core/src/segment_format.rs` | `SegmentReader`, `SegmentBytes::{Owned,OwnedVec,Mapped,Constructed}`, `SectionEntry`, decoded cache, `materialize_posting`, `materialize_validated_posting`, `retain_posting_owned`, `cached_posting` |
| Dictionaries | `crates/core/src/ordered_block.rs`, `posting_dictionary.rs`, `bigram_dictionary.rs` | `OrderedBlockDictionary::{open,from_constructed}`, `SealedPostingDictionary::{open,from_constructed,with_validation,...}` |
| Segment decode | `crates/core/src/hybrid_segment_codec.rs` | `decode_segment_sealed`, `decode_segment_impl(..., sealed)`, `seal_segment_owned`, `seal_segment_consuming` |
| Status surface | `crates/cli/src/execution.rs`, `crates/cli/src/tui/mod.rs`, `crates/desktop/src/backend/index_lifecycle.rs`, `crates/desktop/src/controller/runtime.rs` | consume `IndexStorageStatus`; display `managed_directory` |
| Tests | `managed_store.test.rs`, `lifecycle_storage.test.rs`, `posting_dictionary.test.rs`, `bigram_dictionary.test.rs`, `segment_format.test.rs`, CLI/desktop storage tests | lock, manifest, flush, restore, corruption |

Also read `docs/development/hybrid_index_storage.md`; it is part of the contract and must be updated.

## 4. Change A — generation directories and grace GC

### 4.1 Required layout

```
<managed-directory>/
  store.lock                          # writer + GC only; readers never open it
  manifest.bin                        # atomic pointer to exactly one generation
  generations/
    0000000000000001/                 # immutable once published
      catalog.bin                     # optional; OBJHCAT1
      segments/
        segment-...idx                # immutable, hard-linked when reused
    0000000000000002/
      ...
```

- `manifest.bin` is the only mutable file. Its atomic rename is the publication point.
- A generation directory is complete and synced **before** the manifest names it.
- Segment files are never modified. Reused segments are hard-linked from the previous generation directory into the new one; they are not copied.
- Catalog files are per generation. `catalog.bin` is written fresh each commit.
- Names remain store-generated. Segment names keep the existing pattern `segment-<generation:016x>-<index:08x>-<nonce:016x>.idx`; uniqueness is per generation, so a hard link can retain the source name.

Required constants, documented at their definitions:

```rust
const RETAINED_GENERATIONS: u64 = 8;            // current + 7 previous
const MIN_RETENTION_AGE: Duration = 120s;       // additional safety for slow opens
const MAX_OPEN_RETRIES: usize = 3;
const GENERATIONS_DIR: &str = "generations";
```

Rationale for both count and age: a background flusher can publish several generations within seconds. Count alone can expire a generation during one slow open of a large store. Age alone can retain unbounded garbage under a fast flusher. The combination bounds both.

### 4.2 Manifest version 2

Introduce a manifest-specific version. Do not overload the segment `FORMAT_VERSION`.

```rust
const MANIFEST_MAGIC: &[u8; 8] = b"OBJHMAN1";
const MANIFEST_VERSION: u32 = 2;      // was 1, implicitly FORMAT_VERSION
```

Layout additions relative to v1:

- `manifest_version: u32` (2)
- `segment_format_version: u32` (current segment `FORMAT_VERSION`, 1)
- `model_version: u32`
- repository and configuration identities, canonical root (unchanged)
- `generation: u64`
- catalog record: fixed name `catalog.bin`, length, SHA-256 (unchanged semantics)
- segment count and one record per segment:
  - bare name (`u16` length + UTF-8; no path separator)
  - file length
  - collection ID
  - segment ordinal
  - **`validation_version: u32`** (0 = unvalidated; see Change B)
  - active mask (compact all-live or checked portable Roaring)
- trailing SHA-256 over the payload, as today.

Decoder rules:

- Accept `MANIFEST_VERSION == 1` only for the migration path in Section 4.7.
- Reject unknown manifest versions as `Incompatible`, not `Corrupt`.
- Keep size caps (`MAX_MANIFEST_BYTES`, segment count, name validation) before any allocation.
- Add `segment_format_version` checks so a manifest can never describe a segment format the reader does not support.

### 4.3 Reader protocol: no lock, retry on vanished generation

`ManagedStore::read_generation` must not call `open_lock_read` or `lock_shared`.

```
fn read_generation(&self) -> Result<Option<OpenedGeneration>, StoreReadError> {
    for attempt in 0..MAX_OPEN_RETRIES {
        let manifest = self.read_manifest_typed()?;   // one atomic read
        let Some(manifest) = manifest else { return Ok(None) };
        match self.open_generation(&manifest) {
            Ok(opened) => return Ok(Some(opened)),
            Err(OpenError::GenerationVanished) if attempt + 1 < MAX_OPEN_RETRIES => continue,
            Err(OpenError::GenerationVanished) => {
                return Err(StoreReadError::Conflict(
                    "generation vanished repeatedly during open".into()));
            }
            Err(other) => return Err(other.into_store_error()),
        }
    }
    unreachable!()
}
```

`open_generation`:

1. Form `generations/<generation:016x>` and require an owned, non-symlink directory (reuse existing ownership helpers; reject group/other-writable).
2. Read `catalog.bin` with nofollow + exact length + SHA-256.
3. For each manifest segment: open nofollow, verify exact length, then:
   - `OnDemand` → `SegmentReader::from_index_file_typed`
   - `FullyResident` → `SegmentReader::from_index_file_owned_typed`
4. Apply Change B's trust decision per segment (Section 5.4).
5. Distinguish a missing generation directory or missing named file **for the generation read from this manifest** as `GenerationVanished`; a missing file in a store where the manifest has not changed means corruption, not a race.

Contract statement for the docs: after `read_generation` returns, pinned readers own their bytes and remain valid even if the writer later deletes the generation directory. Grace plus open retry is the safety mechanism; there is no per-reader reference count or marker file.

### 4.4 Writer protocol: build, sync, rename, publish, collect

`commit_generation_inner` keeps the exclusive writer lock. It must not truncate or rewrite a published generation.

Required sequence:

1. `open_lock_read()`; `lock()` exclusive. A second writer receives `StoreReadError::Conflict`.
2. Re-read the current manifest and enforce the expected-generation compare-and-swap and monotonic generation, as today.
3. Create `generations/.tmp-<generation:016x>-<nonce:016x>/segments/` with private permissions.
4. Write and `sync_all` the new catalog as `tmp/catalog.bin` (if any).
5. For each input:
   - `Existing { name, active }`: verify the name exists in the previous manifest, verify length and complete sections under the previous generation directory, then `fs::hard_link(previous/segments/<name>, tmp/segments/<name>)`. Update the active mask in the new record; carry the previous `validation_version`.
   - `New` / `NewStream`: create the file inside `tmp/segments/`, write, `sync_all`, verify length and complete sections. Set `validation_version` only when the input is marked validated by its producer (Section 5.5).
6. `sync` the `tmp/segments` directory and the `tmp` directory.
7. `rename(tmp, generations/<generation:016x>)`; `sync` the `generations` directory.
8. Encode the v2 manifest, write it to `manifest-<nonce>.tmp` in the store root, `sync_all`, `rename` to `manifest.bin`, `sync` the store root.
9. Run grace GC (Section 4.6) while still holding the writer lock.

Failure rules:

- Any failure before step 7 leaves the previous manifest and generation intact. Remove the temporary directory best-effort; never fail the commit because cleanup failed.
- Any failure after step 7 but before step 8 leaves a complete but unreferenced generation directory. GC removes it later as an orphan.
- A hard-link failure aborts the commit before publication with `StoreReadError::Io` and an actionable message. The store requires hard-link support on the store filesystem and is documented accordingly.
- On Windows, deletion of a generation whose files are still mapped may fail. Reclamation errors are recorded in `CommitStats` and retried on later commits; they must not fail an otherwise successful publication.

### 4.5 Reuse accounting

`CommitStats` additions:

```rust
pub generations_removed: usize,
pub generation_bytes_reclaimed: u64,
pub segment_links_created: usize,
pub orphan_generations_removed: usize,
```

A reused segment must increase its inode link count, not store bytes. Add a test asserting that two consecutive generations share the same inode for an unchanged segment and that reported `segment_bytes_written` excludes reused bytes.

### 4.6 Grace garbage collection

Under the writer lock, after manifest publication:

1. List `generations/`.
2. Ignore and report entries that are not owned non-symlink directories with a strict 16-hex-digit name.
3. Delete a directory only when **all** hold:
   - its number is less than `current - RETAINED_GENERATIONS`;
   - its modification time is at least `MIN_RETENTION_AGE` old;
   - it is not the current generation;
   - it belongs to the current user and is not group/other-writable;
   - every entry inside is an owned regular file, an owned directory, or a known temp entry. On any symlink, unknown file type, or foreign-owned entry: skip the whole directory and record a reclaim error.
4. Remove files, then directories, and sync `generations/`. Count bytes before deletion.
5. Additionally remove orphan `generations/.tmp-*` directories older than `MIN_RETENTION_AGE`, and complete generation directories that are newer than the manifest generation (crash between steps 7 and 8), using the same ownership checks.
6. Never delete `manifest.bin`, `store.lock`, or anything outside `<managed-directory>/generations` in this routine.

Bound the work: at most `64` directories examined per commit; anything beyond is left for the next commit and reported.

### 4.7 Migration from the flat v1 layout

At the recorded head, existing stores have flat `manifest.bin` v1 plus `catalog-*.bin` and `segment-*.idx` in the store root.

`ManagedStore::open`/`read_generation` behavior:

1. If `manifest.bin` decodes as v1, attempt migration under the exclusive writer lock.
2. Re-read under the lock; if the manifest is still v1:
   - create `generations/<generation>/segments/`;
   - hard-link each referenced flat segment into the generation directory, verifying name, length, ownership, magic and complete sections;
   - hard-link or copy the referenced flat catalog into the new generation as `catalog.bin` after exact length and SHA-256 check;
   - write a v2 manifest with `validation_version = 0` for every segment;
   - sync everything, atomically replace `manifest.bin`, sync the root.
3. After successful publication, reclaim obsolete flat segment/catalog files using the existing magic/ownership/name checks (`reclaim_orphans` semantics): only positively identified `OBJHSEG1` segments and `OBJHCAT1` catalogs not referenced by the new manifest. Leave unknown files, symlinks, and foreign-owned entries untouched.
4. If the writer lock cannot be acquired, or any linked file fails validation, return `Incompatible` with the reason and let the caller rebuild from source. A cache may be rebuilt; never open a half-migrated store as if it were current.
5. Migration is not a completion requirement for read-only stores; document that read-only v1 stores are rejected and rebuilt.

Migration sets no validation stamps. The first v2 open pays the full validation and may upgrade stamps (Change B).

### 4.8 Reader/writer concurrency guarantees

- One writer at a time; readers never block and are never blocked by the writer during query.
- A reader may race a publication: it sees manifest N or N+1, never a partial manifest.
- A reader may race GC: a vanished generation directory triggers a bounded re-read. If the new manifest names a different generation, retry. If it names the same generation and files are missing, report corruption.
- Pinned readers survive arbitrary later publications on Unix via open file descriptors/mappings. On Windows, reclamation may be deferred; correctness does not depend on prompt deletion.

### 4.9 Alternatives considered and rejected

- **Keep the shared reader lock.** Rejected: the user selected removal; the lock couples slow opens to writer progress and adds a cross-process primitive to every open.
- **Reader pin markers inside the generation directory.** Rejected for complexity and abandoned-marker handling.
- **Single manifest inside a generation directory with a `CURRENT` symlink.** Rejected: symlink portability and ownership rules; atomic rename of one file at the store root is simpler.
- **Shared content-addressed segment pool with reference counting.** Rejected for this change: it requires scanning retained manifests for reclamation and weakens the "generation directory is self-contained" property. Hard links provide the same byte sharing with simpler GC.

## 5. Change B — persistent validation stamps

### 5.1 Trust model

The store distinguishes:

- **Trusted construction** (in-process writer): proofs are carried through the constructed path; bytes never leave the process before publication.
- **Trusted reopen**: exact segment bytes were fully semantically validated once and a stamp records it.
- **Untrusted external input**: any other segment bytes. Full semantic validation is mandatory before absence can affect a result.

The threat model is accidental corruption, torn writes, truncation, stale stores and format drift. It is not an attacker who can rewrite the store while recomputing checksums; such an attacker can already forge the manifest.

### 5.2 Validation levels

Define one enum in the segment layer:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SegmentTrust {
    /// Produced by the in-process writer; writer proofs cover dictionaries.
    Constructed,
    /// Reopened bytes with a matching manifest validation stamp.
    Stamped,
    /// Reopened bytes without a usable stamp. Full semantic validation required.
    Untrusted,
}
```

What each level changes:

| Check | Constructed | Stamped | Untrusted |
| --- | --- | --- | --- |
| Header, magic, section directory, bounds, overlap, required sections | always | always | always |
| Section SHA-256 | on access | on access | on access |
| Record-slot, sort, relation metadata framing/domain | writer proof | yes (cheap structural checks) | yes |
| Dictionary leaf offset tables and descriptor bounds | writer proof | yes | yes |
| Typed order inside and across leaves, duplicate keys, fence keys | writer proof | **skipped** | yes |
| Posting checksum, Roaring framing, cardinality, domain subset | writer proof | on first/uncached access | on first/uncached access |
| Coverage/stats consistency | writer proof | yes (bounded) | yes |

The stamp covers only the eager open-time semantic validation above. It never claims that posting payloads are valid; postings keep their per-use checks.

### 5.3 Validator versioning

```rust
/// Increment whenever the semantic validation rules, dictionary ordering
/// contract, encoding interpretation, or collation assumptions change.
/// Stamps with another value are ignored and trigger one full validation.
const VALIDATOR_VERSION: u32 = 1;
```

Also invalidate on: segment `FORMAT_VERSION` change, `MODEL_VERSION` change, manifest `segment_format_version` change, segment name/length change, or a missing record. A stamp is never reused for different bytes.

### 5.4 How trust reaches the decoder

Do not put a caller-supplied boolean on a public API and do not let arbitrary bytes be declared trusted.

1. `ManagedStore` computes `SegmentTrust` per manifest record:

```
trusted = record.validation_version == VALIDATOR_VERSION
```

2. `OpenedGeneration` gains:

```rust
pub validation: Vec<SegmentTrust>,
pub generation_directory: PathBuf,
```

3. `restore_generation` passes each `SegmentTrust` to `decode_segment_sealed`.

4. `decode_segment_impl(..., trust: SegmentTrust)`:
   - `Untrusted` behaves as today (`validate_components`, full leaf validation).
   - `Stamped` uses the trusted dictionary opener and skips only the order/fence traversal; keeps all bounds, span, descriptor and stats checks.
   - `Constructed` keeps the existing constructed behavior.

5. Dictionary openers get explicit constructors. Keep the trust decision inside the segment layer and document the invariant:

```rust
impl<K> OrderedBlockDictionary<K> {
    /// Full validation. Required for every external byte source.
    fn open_checked(...) -> Result<Self, String>;
    /// Skips leaf ordering/fence traversal. Only ManagedStore may select this
    /// for bytes covered by a matching validation stamp or the private writer.
    fn open_with_proven_order(...) -> Result<Self, String>;
}
```

`SealedPostingDictionary` gets the same pair (`open_checked`, `open_with_proven_dictionary`). `from_constructed` remains the writer-only entry and keeps its current proof semantics. Do not expose these as `pub`.

6. `SegmentReader` itself stays trust-neutral. The same reader may feed dictionaries opened under different proofs during tests; do not store a mutable "trusted" flag in it.

### 5.5 Writing stamps

On commit:

- `New`/`NewStream` inputs gain an explicit provenance flag:

```rust
pub(crate) enum SegmentOrigin {
    /// Writer carried construction proofs for these exact bytes.
    Constructed,
    /// Writer itself decoded and validated these bytes before writing.
    Validated,
    /// Bytes were supplied without validated semantics. Do not stamp.
    Unvalidated,
}
```

- `Constructed` and `Validated` set `validation_version = VALIDATOR_VERSION`.
- `Unvalidated` sets 0 and the next open performs full validation.

On reopen:

- If any segment is `Untrusted`, perform full validation for that segment.
- After the whole generation has been validated and published successfully, call a new store operation:

```rust
pub(crate) fn record_validation_stamps(
    &self,
    generation: u64,
    segments: &[(String, u64, u32)],  // name, length, validation_version
) -> Result<bool, StoreReadError>;
```

Behavior:

1. Take the exclusive writer lock.
2. Re-read the manifest. If its generation advanced, return `Ok(false)`; never rewrite a newer manifest with older facts.
3. Verify each named segment still exists with the same length; set only records whose `validation_version == 0`.
4. Write the same generation with the updated stamps atomically (same manifest-publish protocol). This is a metadata-only manifest replacement; it does not create a generation directory.
5. If the store is read-only, the lock is unavailable, or the write fails, return a typed error that the caller logs and ignores. **A failed stamp write must not fail or slow the open beyond the attempt; correctness does not depend on it.**
6. Never publish a stamp for a segment whose validation returned an error.

Status/diagnostic reporting additions:

- `segments_full_validated: usize`
- `segments_stamp_trusted: usize`
- `validation_stamps_written: usize`
- `validation_stamp_error: Option<String>`
- CLI `run_complete.index_storage_status` includes these fields; the desktop may omit them from the main panel but should keep them in its storage diagnostic.

### 5.6 Failure and invalidation matrix

| Scenario | Required behavior |
| --- | --- |
| Stamp matches, bytes intact | Fast open; postings still checked on use |
| Stamp matches, section bytes altered | Section SHA-256 error on access; no query result |
| Stamp matches, posting payload altered | Posting checksum error; never an empty posting |
| Stamp missing (0) | Full validation; stamp only on success |
| Stamp from older `VALIDATOR_VERSION` | Full validation; then upgrade |
| Segment length differs from manifest | Corrupt; do not use the stamp |
| Manifest generation advanced during stamp write | No-op; keep the newer manifest |
| Read-only store | Full validation each open; report the skipped stamp write |
| Crash during stamp write | Old or new manifest, both complete; no segment changes |
| Migration from v1 | Stamps = 0; first open validates fully |
| Validation fails | Error; no stamp; query not served from that generation |

### 5.7 Why this keeps negative answers sound

A dictionary's absence is a proof only if its bytes and ordering are sound. For reopened stores:

- A matching stamp asserts that this exact byte identity was fully checked by exactly this validator version.
- A corrupted section fails its checksum before it can produce an absence.
- A segment whose stamp is absent or stale is fully checked before use.
- A stamp is never inferred from a filename, a manifest version, a length, or a previous process's success without recording the validator version.

Document that `VALIDATOR_VERSION` must be bumped with any semantic change. Add a comment beside every validation rule module pointing at it, and include the constant in benchmark manifests so evidence identifies the validator.

## 6. Change C — posting proofs without a per-dictionary mutex map

### 6.1 Problem statement

`SealedPostingDictionary.validated` is a `Mutex<HashMap<PostingDescriptor, Arc<OnceLock<Result<(), String>>>>>`. Every posting access takes the mutex, hashes the descriptor, clones an `Arc`, and then consults the `OnceLock`. This is on the query hot path and contends across query workers. It also retains state for every posting ever touched, independently of the decoded-posting cache.

The goal is: validate each immutable payload's semantics before it can affect a result, share work where it is cheap, and remove the lock and hash from the common path.

### 6.2 Required model

- **Constructed / writer-proved payloads**: no external validation (existing behavior; proof is private to the writer).
- **Cache hit**: the cached `Arc<RoaringBitmap>` was validated before insertion. A hit is proof.
- **Cache miss, payload ≥ `MIN_CACHED_POSTING_BYTES`**: call the checked materialization once, then insert into `DecodedPostingCache`. The cache entry carries the proof implicitly.
- **Cache miss, payload < `MIN_CACHED_POSTING_BYTES`**: materialize and validate on each access. These payloads are tiny; the cost is proportional to length and avoids shared mutable state. Do not allocate a side table for them.
- **Eviction**: dropping a cache entry drops its proof; a later access revalidates. This is correct and bounded.
- **Errors**: a failed validation returns an error on every access. Never convert it to an empty bitmap, an absent key, or a missing gram.

Remove `validated`, `with_validation`, and the `constructed`-only shortcut's use of that map. Keep a `constructed` flag (or `SegmentTrust::Constructed`) to select unchecked decode for writer-proved payloads.

### 6.3 Required function changes

In `posting_dictionary.rs`:

- Delete the `validated` field and `with_validation`.
- Rewrite `materialize_descriptor`, `intersect_descriptor`, `intersect_descriptor_into`, `union_descriptor_into` to:
  1. consult `reader.cached_posting(descriptor)` (borrow `Arc`, no bitmap clone);
  2. on hit, perform the requested operation;
  3. on miss and small payload, call `reader.materialize_posting(descriptor, &self.allowed)` and operate on the result;
  4. on miss and large payload, validate/materialize once, then `retain_posting_owned` and operate.
- `get`, `cursor_range`, `visit_bounds`, `materialize_all`, `union_key_into` and any other descriptor consumer must follow the same rule.
- Keep `descriptor.cardinality` usable as a planning estimate before validation, as today; never use it as a proof of an exact result.

In `segment_format.rs`:

- Keep `materialize_posting` (checked) and `materialize_validated_posting` (unchecked) but ensure the latter is only reachable for constructed/writer-proved payloads or after a cache hit.
- Rename or document `materialize_validated_posting` so its precondition is explicit. Do not leave a path where an uncached external posting is decoded unchecked.
- Keep `retain_posting_owned` duplicate-insert handling (returns the existing `Arc` on race).
- Optional, only after measurement: change `DecodedPostingCache` from `Mutex` to `RwLock` and/or replace the recency `BTreeMap` scan with a clock/O(1) eviction. Do not bundle this with the proof removal commit.

In `bigram_dictionary.rs`:

- The sealed gram reader uses `SealedPostingDictionary<u64>`; it inherits the new policy. Ensure candidate/estimate paths still surface errors rather than treating a failed gram lookup as absent. Add a focused corruption test if one does not already cover the new path.

### 6.4 Concurrency semantics

- Two workers may validate the same descriptor concurrently. Both may decode; the cache insert is idempotent (`retain_posting_owned` returns the existing entry). This is correct and avoids a global lock.
- No lock is held while decoding, hashing or domain-checking.
- A validation failure is deterministic for immutable bytes. Concurrent failures each return the error; do not cache errors in a way that could hide a transient I/O error. Distinguish I/O errors from content errors: only content errors are immutable for a given byte span; `segment_format` currently returns `String`, so keep the existing error strings and do not attempt a new error taxonomy in this change.
- The `allowed` domain is immutable per dictionary; a cache entry created for one dictionary's domain must not be reused by a dictionary with a different domain. Today the cache is per segment and keyed only by descriptor. Confirm that all dictionaries in a segment share the same eligible domain or make the cache key `(descriptor, domain identity)`. If domains differ, key the cache by `(descriptor, domain fingerprint)` or store the domain with the entry and compare. This is a correctness prerequisite, not an optimization.

### 6.5 Measurement gate for a dense descriptor table

Do not implement a dense `Box<[AtomicU8]>` state table yet. After the cache-proof change, measure:

- posting accesses by payload size bucket,
- repeated validations of sub-64-byte payloads,
- time in validation vs materialization,
- Q167/Q175/Q177 and dictionary-heavy cases.

Add the dense table only if small-posting revalidation accounts for a material share of query time. If added, require a format-level descriptor ordinal (writer-assigned, validated on decode) so the table can be sized without a hash map; document memory (1 byte or 2 bits per posting) and include it in the cache/residency accounting. Do not key a dense table by raw payload offset.

## 7. Correctness and failure-injection tests

### 7.1 Store layout and GC

1. Publish generation 1, then 12 more; assert the current and previous 7 directories remain, older ones are removed only after the age threshold, and no referenced file is missing.
2. Assert unchanged segments share an inode across generations and that `segment_bytes_written` excludes links.
3. Open a generation while a writer publishes 10 generations; the reader either opens the old generation or retries to the new one; never returns a partial generation.
4. Hold an exclusive writer lock; assert a reader still opens the current generation without waiting.
5. Kill a writer between generation-directory rename and manifest rename (simulate by arranging the directory then not publishing); assert the old manifest opens and the orphan is reclaimed later.
6. Interrupted catalog write, interrupted manifest temp, truncated manifest, unknown file in a generation directory, symlinked generation directory, foreign-owned file: each must be rejected or skipped with the documented category, never deleted.
7. Migration: flat v1 store with two segments and a catalog migrates to v2; results identical; flat leftovers reclaimed only after the v2 manifest is published; a missing flat segment yields `Incompatible`, not a partial store.
8. Read-only store: open works, stamp write reports an error, no deletion attempted.

### 7.2 Validation stamps

9. Stamp matches: open succeeds without leaf-order traversal (instrument a counter); queries match baseline digests.
10. Alter a section byte without updating the stamp: the section hash fails on access; no result is returned.
11. Alter a posting payload: checksum error; FTS/scalar queries return an error, not "no match".
12. Stamp version mismatch: exactly one full validation occurs; the stamp is upgraded; the next open is trusted.
13. Validation failure: no stamp is written; repeated opens fail identically.
14. Stamp write races a newer publication: no-op; the newer manifest remains authoritative.
15. Constructed segments are stamped at commit; externally validated segments are stamped; unvalidated inputs are not.
16. Manifest v2 rejects unknown `segment_format_version` and validator-incompatible stores as `Incompatible`.

### 7.3 Posting proofs

17. Corrupt a posting payload in an external store and assert every scalar, range, structural, length, bigram, FTS and relation path that touches it returns an error.
18. Same corruption in a stamped (trusted) segment: still an error. The stamp does not cover posting payloads.
19. Two threads query the same uncached posting concurrently; results are identical and no panic/deadlock occurs.
20. Force cache eviction; the posting is revalidated and the result is unchanged.
21. Small-posting path: repeated accesses validate each time and remain correct.
22. Cached operand intersections/unions do not clone the cached bitmap (assert with a borrowed-use counter or code review + focused test).
23. Domains: dictionaries with different eligible domains in one segment either share a proven domain or use distinct cache keys; a posting from one domain is never treated as proven for another.
24. Constructed fast path still decodes correctly after the change; existing sealing/constructed tests remain green.

### 7.4 Integration

25. Full workspace tests, Clippy, formatting and the GUI environment example as documented.
26. CLI/TUI/desktop: disk open, memory open, reopen, flush, status JSON, pending/failed synchronization, managed-directory display. Desktop open dialog still shows the store root, not the generation directory.
27. Snapshot/deprecated cleanup still works in the selected scope after the layout change; `OBJSNAP1` v27/28 files are deleted only under the existing narrow checks.

## 8. Measurement and acceptance

Use the established archive conventions. Record binary SHA-256, build revision, execution revision, fixture/schema/workload fingerprints, CPU governor, worker counts, cache settings and raw commands.

Baseline for this work: the recorded branch head at the start (reviewed head `2ba53f3a`). Compare candidate to that baseline, not to historical numbers from `da8fde68` or `e15b3f5`.

### 8.1 Commands

- Construction (7 paired fresh processes): `bench_index_construction --root examples/musenalm --result-dir <dir> --alphabet ascii --workers 32 --incremental`.
- Stored open (on-demand and fully resident): `bench_musenalm --root examples/musenalm --result-dir <dir> --extended --incremental --storage custom <store> --profiles ascii-off --cases Q001 --repetitions 1`, reading `open_ns`, RSS and zero source reads from the manifest. Run 7 trials per mode.
- First flush: `bench_managed_flush --root examples/musenalm --output <store> --alphabet ascii`, 7 trials.
- Queries: the 220-case extended campaign in ASCII and Unicode with five repetitions, verified-result cache disabled; tail campaign with 30 repetitions for Q001/Q008/Q042/Q143/Q167/Q175/Q177/Q194/Q202. Use the archived scripts `matched_tail.py` and `summarize_final.py` unless they need a path adjustment; keep the statistics definition (middle-pair median, nearest-rank p95, geometric mean over paired ratios).
- Disk: store bytes, file count, inode link counts, reclaimed bytes per GC pass; run 32 updates and report retained generation count and garbage.
- Memory: retained/peak RSS for construction and after open; decoded-cache usage; removed validation-map memory.

### 8.2 Expected outcomes and gates

These are engineering targets, not a justification to move work outside a timer:

- **A**: reader open must not block on a writer. No measurable open regression; commit/flush overhead bounded (hard links are O(segments), not O(bytes)). Store bytes must not grow beyond one current generation plus new segments per retained generation; assert no per-generation full copy.
- **B**: fully resident and on-demand open become faster after the first stamped open. Record the first-open full-validation cost and the steady-state stamped-open cost separately. A stamp must never make a corrupt store silent.
- **C**: query geomean and tails must not regress; expect a small improvement from removing the mutex/hash/Arc on every posting access. Report per-size-bucket validation work.
- Overall: no result digest changes, no failed samples, no verified-result-cache hits. Preserve the current task policy from the 13 November repair checkpoint: report small differences honestly, do not claim absolute completion gates passed, and do not hide a regression behind another improvement.

## 9. Commit sequence

Each commit is self-contained, tested and pushed immediately.

1. **Instrumentation and baseline** — add counters for full validation, stamp trust, leaf traversal, posting validation by size, cache hits/misses, generation GC bytes, hard-link counts. Archive the baseline measurements under `bench/results/hybrid-index-storage/maintenance-20261114/` or a new descriptive directory. Do not alter query behavior.
2. **Change C** — remove the per-dictionary validation map; cache proof; small-posting policy; tests; focused query measurements. (May be a `task/hybrid-index-storage/posting-proof` subtask merged by the supervisor.)
3. **Change B, format part** — manifest v2 with `validation_version` and `segment_format_version`; segment origin flag; trust plumbing (`SegmentTrust`, `open_checked`/`open_with_proven_order`); `record_validation_stamps`; status fields; tests; first open/stamped open measurements.
4. **Change B, migration** — flat v1 to v2 migration with zero stamps; reclaim rules; tests.
5. **Change A** — generation directories, lock-free reader, hard-link reuse, grace GC, orphan cleanup; reader/writer concurrency tests; open/commit/disk measurements.
6. **Documentation and final campaign** — update `docs/development/hybrid_index_storage.md` (layout, reader protocol, grace contract, stamps, validator version, posting proofs, GC constants, hard-link requirement); update CLI/TUI/desktop status documentation; run the full campaign; write the report; keep the task `incomplete` unless the supervisor determines all outstanding repairs are integrated and validated.

If Change C lands first, Change B's manifest bump must not invalidate its cache semantics. If Change B lands first, Change C must not depend on the validation map. They touch disjoint code except for tests.

## 10. Documentation updates (required)

`docs/development/hybrid_index_storage.md`:

- Replace the flat-layout description with the generation-directory layout.
- State that readers do not lock and explain grace + retry.
- State the hard-link requirement and bounded-garbage property.
- Define `MANIFEST_VERSION`, `VALIDATOR_VERSION`, stamp semantics, the exact checks skipped under a stamp, and the checks that remain.
- State that stamps cover open-time metadata/dictionary semantics, not posting payloads.
- Document the posting-proof model: cache hit = proof; small payloads validate per access; errors are never absences.
- Update the validation evidence table and the "Only one writer… readers hold a shared lock…" paragraph.
- Document migration from v1 and read-only behavior.

CLI/desktop docs and JSON status additions must be reflected in `docs/cli/obj.md` if the status object is documented there.

## 11. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Grace window expires during a very slow open | Retain 8 generations, require 120 s age, retry 3 times, distinguish vanished vs corrupt; document the probabilistic contract |
| Hard links unsupported/fail | Abort commit before publication; previous generation remains; document filesystem requirement; no copy fallback that could exhaust disk |
| Deleted generation while a reader maps it | Unix open-file semantics; Windows deletion failure recorded and retried; never correctness-dependent |
| Stamp lets a corrupt store through | Stamp bound to validator version, segment identity/length, manifest checksum; section/posting checksums remain; full validation on any mismatch |
| Developer changes validation without bumping `VALIDATOR_VERSION` | Comment on every rule module, include constant in manifests/reports, add a review checklist item; optional unit test asserting the constant appears in the validation module docs |
| Concurrency race validating the same posting | Idempotent cache insert; no lock held during decode; deterministic content errors |
| Cache proof shared across different domains | Verify domain equality or key the cache by `(descriptor, domain fingerprint)` |
| Scope creep into lock-free multi-writer | Keep the exclusive writer lock; explicitly reject a second writer |
| Benchmark mix-up between backends | Match baseline and candidate on the same backend, cache settings and fixture; record both build and execution revisions |

## 12. Deliverables and completion

Required artifacts:

1. Implementation commits on `integration/hybrid-index-storage` with immediate pushes.
2. Updated `docs/development/hybrid_index_storage.md` and affected CLI/desktop status docs.
3. A report under `bench/results/hybrid-index-storage/maintenance-20261114/REPORT.md` (or a clearly named successor) containing:
   - reviewed and final revisions, commands, binary hashes, fixture/schema/workload fingerprints;
   - Change A layout, GC constants, disk usage/link evidence, reader-race tests;
   - Change B stamp format, first-open vs stamped-open timings, validation hit counters, corruption results;
   - Change C posting-access distribution, query medians/p95s, cache behavior, removed map memory;
   - before/after construction, open, flush, query and memory tables;
   - exact remaining limitations and why any unmet gate remains unmet.
4. A disposition table for A/B/C: implemented, verified, incomplete, with code references.
5. Task documents: keep the authoritative task in `docs/ai/tasks/` until the supervisor confirms the complete `hybrid-index-storage` scope is integrated and validated. Append the implementation report and move to `done/` only per `docs/ai/README.md`. Do not merge `master`.

Definition of done for this plan specifically:

- Readers take no lock and survive concurrent publication/GC under the documented contract.
- Store bytes are shared via hard links; GC is bounded by count and age; no user or unknown file is ever deleted.
- Stamped reopens skip only the proven-expensive checks; corrupt bytes still fail loudly; stamps are never written for unvalidated bytes.
- Posting access has no per-dictionary mutex/hash/Arc on the hot path; corruption still surfaces as an error; two-thread and eviction tests pass.
- All workspace tests, Clippy, formatting, CLI/TUI/desktop storage tests and the full query campaign pass with unchanged results.
- Measurements are archived with provenance and honestly report any remaining regression.


## Implementation report

- **Task identifier and closure date:** `hybrid-index-storage` generation/validation continuation, 2026-11-14.
- **Base and integration:** Actual starting head `ca183a1ed8` (the plan's reviewed head was `2ba53f3a20`). Work landed on `integration/hybrid-index-storage` in `/home/simon/source/objekte-integration-hybrid-index-storage`. The original checkout stayed on `master`.
- **Implemented:** Change A publishes immutable generation directories, hard-links unchanged segments, uses a lock-free reader protocol with retry, and applies age/count grace collection. Change B persists validator-versioned stamps so matching reopens skip established eager checks while framing, bounds, section/posting checks and untrusted first-open validation remain. Change C removed the per-dictionary validation mutex map and reuses eligible posting proofs; the subsequent query series changed decoded-cache read hits to atomic clock bits. Implementation commits include `c977710f2d`, `8ca2851ab8`, and `4a1c2a8883`.
- **Validation:** Generation, migration, inode-sharing, concurrent-reader/GC, stamp invalidation, corruption, cache-eviction, and posting-domain tests passed in the workspace suite. Combined revision `74cab22411` passed `cargo test --workspace --locked --quiet` and `cargo clippy --workspace --all-targets --locked --quiet` with Clippy warnings. Global formatting checks still show existing drift.
- **Benchmarks:** The [generation checkpoint](../../../bench/results/hybrid-index-storage/maintenance-20261114/REPORT.md) retains exact seven-pair build/flush/open commands, binary hashes, raw measurements, and three paired 220-case query campaigns with zero verified-result-cache hits and identical digests. Stamped reopens were about 4–5% faster than deliberately unstamped v2 first opens; the new first flush measured 1.088× its comparison. The [final query campaign](../../../bench/results/hybrid-index-storage/query-optimization-20261115/REPORT.md) contains the later posting-cache evidence and matching digests.
- **User-approved deviation and remaining work:** The user waived further performance evidence and asked for documentation closure with outstanding work in issues. The complete foreign-owned/dictionary-corruption failure matrix and lower-level typed-error propagation are [tracked here](../issues/feat/hybrid-store-failure-validation-coverage.md); their omission is not represented as a passing test. The integration branch/worktree are retained. The query subtask worktree was merged, pushed, and removed.
