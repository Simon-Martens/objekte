# Objekte implementation specification · Direct ingestion and managed file repairs

## Mandate and starting revision

Implement both repair passes on one new integration branch. Part I corrects the earlier direct-ingestion repair. Part II corrects Assignment 2, the optional FileCatalog and callback import route. A successful result lets an application submit fileless records and import files into the same collection while searches continue against consistent index revisions. The important failures to eliminate are incorrect revisions, lost accepted work, partial file visibility, incorrect membership counts, repeated decoding or copying, and work proportional to unrelated files or records.

| Item | Value |
|---|---|
| Repository | `Simon-Martens/objekte` |
| Start branch | `origin/integration/core-managed-file-ingestion` |
| Reviewed start | `f5b846771ccf63151ad33f07739f5a19dcb26505`, with production code at `bf34c076e11e5b9cc94f9f635e366d9d9de0c749` |
| Earlier repair already included | `3bf0811943b11837bf997f51ee9f3392e0670e99`, with production code at `732725bcfba34e11f6b85b0125e0e447151fb03a` |
| Integration branch | `integration/ingestion-publication-and-resource-repairs` |
| Sibling worktree | `/home/simon/source/objekte-integration-ingestion-publication-and-resource-repairs` |
| Completion evidence | `docs/ai/generated/ingestion-publication-and-resource-repairs/` and the completed task under `docs/ai/done/` |

Read `AGENTS.md` and `docs/ai/README.md` first. Fetch the start branch, check its tip and verify that it descends from the earlier repair. If the tip has advanced, check the named functions and regression expectations on that tip. Give existing correct work credit and record the actual base. Resume an existing task worktree if it already contains work on this assignment. From the normal repository checkout, the commands for a fresh worktree are:

```bash
git fetch origin integration/core-managed-file-ingestion
git merge-base --is-ancestor 3bf0811943b11837bf997f51ee9f3392e0670e99 FETCH_HEAD
git worktree add -b integration/ingestion-publication-and-resource-repairs \
  /home/simon/source/objekte-integration-ingestion-publication-and-resource-repairs FETCH_HEAD
cd /home/simon/source/objekte-integration-ingestion-publication-and-resource-repairs
git rev-parse HEAD
```

Work against this combined baseline for both parts. Each implementation commit goes to the integration branch. Leave the original checkout, submitted task documents and master under the repository’s normal workflow. External JSON/JSONL packages and moving JSON Schema ownership belong to Assignment 3. This repair supplies the file-reader contract and performance on which that later work depends.

## Contract the implementation must satisfy

| Concept | Meaning in this assignment |
|---|---|
| Accepted direct batch | Submission transferred ownership and returned a receipt with a session sequence. A successful receipt means the entire batch is visible. A failed admitted batch has no visible operations. Earlier successful batches survive. Later unpublished receipts identify the originating failure sequence. |
| Independent publication | A different writer advanced the index revision without invalidating the staged operation’s registration, ownership, logical record preconditions or observed file. Both writers’ changes must survive. A logical key includes its collection. |
| File observation | A root-contained path, catalog identity where allocated, filesystem object identity where supported, size, modification time and available creation time observed on the file being decoded. File version identifies the catalog incarnation. Neither the index revision nor a provider record version substitutes for this evidence. |
| File revision publication | All selected collections’ records, ranges, postings, diagnostics, reverse routes and applicable file metadata become visible in the same root installation. A pinned old root retains its previous view. A successfully parsed empty file is distinguishable from a missing or pending file. |
| Parallel work | Callback order across files is unspecified. A registered reader may be called concurrently. Within one file the format identifies safe decode boundaries. Independent batches may execute concurrently under the shared worker budget. Core assigns slots, builds segments and publishes the revision. An ordinary serial reader remains usable. |
| Fileless data | Direct retained/provider records have no catalog membership, file range or system file metadata. A provider may supply original bytes under its own version contract. An ordinary application field named `path` or `@file` has no special effect. |
| Resource bound | Pending input, resident waiting descriptors, active jobs and completed intermediate work waiting for admission have explicit owners and bounds. Finished private index segments necessarily grow with the final index. Keep that memory visible as a separate category. |

Use paths below relative to `crates/core/src/` unless a path begins with `crates/` or `docs/`. Suggested type names and test names are illustrative. The described behavior and test outcomes are the acceptance criteria.

## Work order

| Checkpoint | Implement | Evidence before advancing |
|---|---|---|
| A | I.1 · direct publication across unrelated revisions | Both writer orders, real collision, exact receipt sequence |
| B | I.2 and I.3 · group ownership, spool descriptors and receipt retention | Multi-batch group success/failure, descriptor cap, reclamation |
| C | II.1 · same-handle input and final source verification | Initial build, add/update, conditional bytes, rebuild races |
| D | II.2 · segment capacity and unique collection memberships | Exact segment and mutation counts |
| E | II.3 · physical-file publication across collections | Paused reader, late failure, pinned and current views |
| F | II.4 · generic streaming and shared scheduling | Pre-EOF private segment, one-worker progress, concurrent decoding and rollback |
| G | II.5 · catalog scalar maintenance and routed evaluation | Correct selective and broad queries after churn, zero content reads |
| H | Integrated validation | Workspace tests, public example, workload comparison and pushed evidence |

Build a small deterministic regression as each defect is addressed. Record which assertion fails on the baseline or, for resource measurements, which counter demonstrates the excess work. A checkpoint may share internal helpers with the following checkpoint. Keep every pushed implementation commit buildable. Run short focused tests during development and the full validation once the changes are integrated.

## Part I · Repair the direct-ingestion implementation

### I.1 · Install prepared direct work against the current index

**Trace.** `ingestion/publication.rs::install_batch` reads a view, constructs retained/provider segments, then passes that view’s revision to `lifecycle/publication.rs::publish_record_batch`. The latter rejects a changed global revision before looking up touched IDs. `ingestion/queue.rs::run_worker` treats the ungrouped error as terminal. A file addition between preparation and installation can therefore cancel two perfectly valid direct submissions. The existing test `direct_batch_publishes_while_multisegment_file_update_is_paused` exercises the file finishing second.

**Implementation sequence**

1. Separate reusable prepared content from placement into the current root. Keep the source registration, collection, operations in sequence, relevant ownership/version preconditions, and the privately built segment data. Assign final segment IDs, routes, liveness changes and next revision at installation. The versioned provider’s submitted version travels with the prepared input.
2. Record preconditions only for the touched logical keys and registration. An insert needs a vacant key. Replace/delete need the live registration-owned record the operation was prepared to change. Use existing logical identity/version evidence where available. Physical slot numbers may change under compaction. Test an unrelated compaction without treating a relocated live record as a different logical owner. A real intervening retirement and replacement that invalidates a precondition must still conflict.
3. Build expensive indexes outside the root write lock. At handoff, obtain a fresh pinned view, validate registration and touched preconditions, build the affected routes against that view, then use the lifecycle installation check to swap the root. A losing compare-and-swap invalidates root-relative placement. Keep the prepared records/indexes for another attempt. Reuse the existing writer coordination or add a short serialized installation step if needed for progress under contention. Serializing complete file parsing and direct index construction would block independent work.
4. Preserve the existing cancellation decision boundary. Either the root was installed and the covered receipts receive its revision, or cancellation won and the batch has no visible writes. Register a test hook outside the root write guard. Verify the old pinned root, new root, receipt and mutation report rather than relying on log timing.

**Regression schedule.** Use a programmatic index with its initial construction complete. Register a file reader and direct writer for collection `entries`. Submit direct batch D1 for `manual-a`. Pause D1 immediately after preparation, submit D2 for `manual-b`, import a file containing `file-a`, and wait for that import to publish. Release D1. Before release only the file is visible in that pinned view. After both receipts succeed, the new view returns all three IDs once. Assert D1’s prepared data was built once. Force D1 down the ungrouped path, then exercise a coalesced group.

Repeat with the file claiming D1’s ID and with an already published direct prefix. The file and prefix survive. D1 fails for its real key conflict and D2 is cancelled with D1’s sequence. A fresh session can resume from the successful root. Include a provider-backed input and check the provider read count through publication retries.

### I.2 · Coalesce successful batches without copying their payloads

**Trace.** In the grouping block of `ingestion/queue.rs::run_worker`, decoded members remain in `group` while `combined.operations.extend(batch.operations.iter().cloned())` constructs a second set of operation payloads. Cloning nested strings, arrays, text and field paths raises the memory high-water on the normal route. The originals exist to replay a failed group.

1. Retain each input batch boundary explicitly. Record its sequence, operation range and retained/provider shape. Reuse the current grouping eligibility, record target and byte budget. Keep replace/delete and incompatible registration or representation boundaries ordered. The existing public batch builder rejects repeat operations on one key inside a single batch. Repeated keys across successive batches still follow sequence order.
2. Use a single owner of the group’s input. A practical implementation is to consume ordered members into a bounded group builder that records boundaries and attributes validation errors to the current member. Move retained bodies into final storage. Index provider values once, preserve their version columns and release transient values. Borrowed keys or shared immutable IDs are reasonable where the routes require them.
3. Validate public batch atomicity at the member level. If a member fails while being converted, discard its private work and install any valid earlier members. If a conflict is first detected when installing a prepared group, use the boundary metadata to isolate the maximal complete valid prefix. Segment slot projection or splitting private prepared work on this exceptional path is acceptable. Preserve all index families, including text, sort and relation columns. A good first record of a two-record failing member must never appear.
4. Keep the grouping gain. Compatible singletons from different submissions should share a segment and a root installation. A design that builds a final segment per submission to avoid cloning merely reinstates the earlier bottleneck.

**Small oracle.** Seed one live file record `occupied`. Pause the direct worker and enqueue four compatible batches. Submit [`a`], [`b`], [`c`, `occupied`], [`d`]. Resume. Receipts 1–2 succeed, receipt 3 identifies the duplicate, receipt 4 identifies sequence 3 as its cancellation origin. The file record, `a` and `b` are live. Neither `c` nor `d` exists. Extend the existing retained and spooled failed-middle-group tests. Also try a corrupt spooled middle frame and a group whose failure coincides with an unrelated file commit. For the success path use large nested strings and explicit text. Use focused allocation/copy instrumentation or a tracked backing buffer to demonstrate that combining members introduced no second deep copy of their payloads. Record segment and root counts alongside allocation evidence.

### I.3 · Bound queue overflow and release obsolete receipt state

**Trace.** `ingestion/queue.rs` caps `State.items` at 65,536 but appends one `OverflowQueued` to a second resident deque for every excess submission. Each item still owns frame coordinates and scheduling data. `ingestion/spool.rs::SpoolManager` already owns the private chunk files, length/checksum handling and physical reclamation. `ReceiptState::results` also appends a terminal span for each completion operation for the lifetime of the session, even after the corresponding receipts have been dropped.

**Spool algorithm**

1. Keep the bounded resident FIFO. For a spilled suffix, persist enough sequential bookkeeping to recover sequence order, grouping eligibility, size charge and frame location. One workable layout is a compact descriptor stream in spool order with a per-chunk cursor and a bounded read-ahead window. Another is a versioned self-describing spool frame with an implicit contiguous ordinal. Choose one. Include all additional on-disk bytes in the reservation. Transient spools can change internal format with versioning and the existing checksums.
2. Preserve one visible FIFO for concurrent producers. Assign a receipt sequence only when its submission can be committed. A reserved or partially written frame is private. When a spilled suffix exists, newly accepted work cannot appear ahead of it merely because RAM has become available. Promote only a bounded prefix into newly freed resident capacity. Worker lookahead, retry buffers and pending write reservations need explicit owners and budgets. Keep the number of resident **waiting** descriptions across queues/windows at or below 65,536. Report actively preparing work separately.
3. Preserve the two public admission behaviors. A memory-only `try_submit` that cannot proceed returns the caller’s original batch through the existing transient refusal. An ordinary `submit` may spill within its quota. Reserve physical space before writing, serialize or otherwise bound concurrent encoders, and keep disk I/O outside the shared admission-state lock. Name the lock order in one short implementation comment. A failed reservation/write or a close racing a write must return the original batch when admission has not committed. Reclaim an incomplete tail, leave earlier frames readable and avoid an accepted sequence gap.
4. Account and reclaim partial chunks correctly. A consumed frame in a still-allocated chunk does not free that frame’s bytes for reuse under the physical spool quota until reclaimed by the storage mechanism. Cleanup handles cancellation, final-owner drop, failed/corrupt frames and normal finish. Keep work confined to the session’s own temporary directory.
5. Bound completed receipt history independently. One implementable option is a modest fixed sequence window per shared completion block, with receipts holding their own block and the producer retaining only open blocks required to finish work. Once terminal, the block may outlive the worker through a caller’s receipt. Drop obsolete session-owned references and empty lookup entries. A single retained early receipt must not keep every later completion resident. Preserve the same result on repeated waits, last-owner drain, finish, successor-session coordination and the flush watermark. Choose a block size or equivalent design that avoids one mutex/allocation per submission on the common path.

**Acceptance**

| Test | Required outcome |
|---|---|
| Paused worker, 100,000 tiny submissions, spool enabled | All accepted within configured quotas, including submissions above 65,536. Record maximum resident waiting descriptors from all queues and actual spill bytes. Increase backlog again and show descriptor memory stays bounded. |
| Four producers, resident queue draining while overflow remains | No duplicate or skipped sequence, no later in-memory bypass, valid original inputs on refusal. Every accepted key appears once. |
| Partial write, checksum failure, quota exhausted, cancel during reservation | Physical disk accounting matches reclaimable storage. No partial record becomes live. Receipts or submission errors have the correct origin. Session-owned storage is released. |
| Long session with repeated submit/flush/drop | Keep one early receipt and a sample of recent receipts. After many later completions, all retained receipts can be awaited twice. Session-owned completed state follows outstanding ownership rather than total completions. |
| Worker close, successor and pinned old root | No two sessions publish the same accepted item. All valid prefix receipts complete before the successor takes ownership. |

Start with the existing queue tests for paused spool overflow, a partial spool frame, failed middle members, cancellation and last-owner drop. The old stress test’s assertion about the length of the overflow deque encodes the defect. Replace it with a behavioral capacity assertion.

## Part II · Repair Assignment 2 managed file ingestion

### II.1 · Obtain metadata and content from the same observed source

**Defects to reproduce.** Ordinary initial construction discovers A, then reads a pathname using `check_freshness = false`. It can parse an equal-length replacement B while retaining A’s catalog revision. The ordinary managed mutation path opens A to observe its identity, then calls `frame_file_for_build(path, …)`, which opens the pathname again to read. If B occupies the path for that second open and A is restored, the final identity check sees A although B supplied the postings. The conditional byte-update implementation makes an extra complete pass before entering path preparation. Programmatic rebuild has a handoff after its private preparation where a changed path is not revalidated.

**Entry-point inventory**

| Entry point | Follow these functions on the reviewed base |
|---|---|
| Small-file initial build | `construction/initial.rs` → `construction/batch.rs::process_batch_profiled` → `source/extraction.rs::extract_file_build_outcomes_hashed_with_buffer` → `extract_file_outcomes_impl`. The fresh-build call passes `false` for the freshness check. |
| Large-file initial build | `construction/file_preparation.rs::process_framed_file_profiled`, including generic streaming, whole-file framing, bounded JSON Lines and fallback. The specialized JSONL path opens the file itself and uses the discovered size in its statistics. |
| Managed add, update and reconciliation | `lifecycle/mutation/preparation.rs::prepare_file_at`, `construction/file_preparation.rs::prepare_file_build`, `lifecycle/mutation/staging.rs`, and the handoff in `lifecycle/mutation/mod.rs` or `lifecycle/reconciliation/mod.rs`. Follow both successfully framed and malformed/unreadable outcomes. |
| Conditional replacement | `lifecycle/mutation/preparation.rs::prepare_replacement_bytes` compares caller bytes to a first read and then calls path preparation. Audit both public `update_file_from_bytes` variants. Preserve old-content evidence used by verified removal as a separate input. |
| Rebuild and pinned rereads | `lifecycle/maintenance/rebuild.rs` prepares files before the final root installation. `source/access/mod.rs` already has checked range and read-session paths. Preserve the same identity/revision guarantees for logical payloads, projections and original bytes. |

**Implement the common read sequence**

1. Validate the source selection and root containment using existing path and symlink semantics. Open the source, then derive the observed revision, available creation time and supported object identity from that handle. Reuse `SourceRevision` and the existing verification code. Discovery selects/schedules a candidate. If the open produces different evidence, refresh the candidate coherently within the retry policy or return the stale outcome. Never combine discovery metadata with differently observed content.
2. Pass that handle into the reader path. For a whole-document reader, read a reusable buffer once, hash on that pass where required, and frame, parse and handle malformed input using those bytes. Preserve the original bytes for raw fallback. For a streaming reader, pass bounded input from the same handle and hash during consumption. The pathname may serve as a diagnostic label. It cannot be an independent source of bytes inside a fallback helper.
3. For conditional bytes, compare the consumed source against the borrowed caller buffer at successive offsets. Check length, early EOF, extra bytes and the first differing byte. Keep partially prepared work private. Return the existing mismatch/stale outcome on failure. When ordinary parsing already requires one contiguous verified buffer, compare and parse that buffer. The successful single-reader route should perform one content pass. A retry for a newly observed revision is a different attempt and needs fresh evidence.
4. Validate the handle after consumption. Retain compact evidence with the prepared file so the publication handoff can check the path still resolves to the observed object, source revision and expected live catalog incarnation. Deduplicate validation by observed file revision across segments. Perform expensive digest fallback outside the write lock. If the platform cannot establish identity, preserve the documented verified-content or explicit unverifiable outcome. Indexing should not keep one open file per item in a large backlog. No implementation can prevent a later external edit immediately after a successful check.
5. Wire that evidence through initial publication, add/update, reconciliation and rebuild. Reuse valid completed preparation over an unrelated direct publication when the binding, source and ownership preconditions still hold. For a new file whose provisional catalog ID collides with another concurrent add, assign/remap an available ID consistently or restart that affected attempt. Never bind its finished record ranges to the other file’s ID. A pinned reread may return the old content if provable, otherwise the existing stale/unavailable result.

**Filesystem regression matrix.** Use two documents with the same byte length, IDs `A` and `B`, different searchable strings, equal modification times and distinct filesystem identities. Create temporary files outside the collection’s discovery pattern before renaming them into place. Use channels or barriers at named events. A sleep followed by a probabilistic rename is inadequate.

| Placement of edit | Observable acceptance result |
|---|---|
| Discover A, replace by B before the ordinary initial read | Complete B with B’s observation after a valid refresh, or a stale/unavailable attempt. A’s metadata with B’s postings is a failure. |
| Open A, put B under its pathname for framing, restore A before handoff | Successful A contains A’s ID, text and source range. An explicitly detected conflict may abort the candidate. Never publish B under A. Count actual content opens on an uncontended ordinary JSON import. |
| Parse A, leave B at the path before publication | Verified retry to B or explicit stale result. The prior published view remains internally consistent. Repeat on the large-file route. |
| Conditional supplied data agrees, disagrees late, or has a different length | Only matching observed source data can commit. A mismatch after earlier emitted chunks leaves the previous view intact. Check public result and file version. |
| Replace the file at the rebuild handoff, after private segments exist | Rebuild retries or fails visibly. The candidate built from superseded evidence cannot appear as a newly verified revision. |
| Replace A after A has already published | Pin the original view. Its indexed facts remain A’s. A lazy payload or preview returns A’s proven bytes or the supported unavailable outcome. A newer view may serve B after a successful update. |

On each successful case, inspect direct ID lookup, a field/FTS query, original bytes, locator bounds, path/ID catalog lookup and file reverse membership from the **same pinned view**. Record separate counts for metadata observations, content opens, content bytes, verification reads and retries. Opening once just to stat and reopening to decode fails the same-handle contract even if only one read was counted.

### II.2 · Make segment limits and catalog membership independent of reader chunks

These are two distinct defects. Fix each with its own small test before running the 100,000-record scale case.

**Segment capacity.** `construction/initial.rs::admit_large_chunk_profiled` and `lifecycle/mutation/preparation.rs::index_prepared_file` check `SegmentAdmission::at_capacity` after a chunk. That function treats reaching `max_files` as a reason to seal immediately. At `max_files = 1`, a continuation of the same file is forced into a new segment. Review `can_admit_batch`, the meaning of `first_range` and `BatchContribution::new_file_count`. A “first range of this source” flag and “new file in this current segment” answer different questions, especially immediately after a segment was sealed.

1. Compute incremental distinct-file cost using file identities already present in the current segment. A continuation costs zero when that file already belongs to the segment. It costs one after starting an empty segment. If an incoming batch includes several files, count its distinct new members once each. Keep the ordinary sequential fast path cheap.
2. Seal when the actual record limit is reached, before a new distinct file would exceed the file limit, at a genuine resource boundary, or when private admission finishes. A segment exactly at the file limit may accept more records from an existing member. Keep current supported whole-record rules for an indivisible oversized input. Splitting generic reader chunks must preserve whole record spans and diagnostics.
3. Use the same policy for initial construction and managed add/update. Small oracle: 10 records from one file, two-record input chunks, target 4, max files 1. Final record counts are 4, 4 and 2. Then add a second file under the same limit and prove no segment contains both. Repeat for max files 2 with interleaved continuations to cover the full-but-still-eligible case.

**Catalog collection counts.** In `index_prepared_file`, `indexed.iter().map(|segment| segment.collection).collect()` stores one collection ID per resulting segment in `FileEntry.collections`, which is a vector. `lifecycle/view.rs::file_record_count` walks all routed segments for every entry in that vector. A three-segment import in one collection can therefore visit its records three times. A deduplicated returned key list can conceal the error in counts and update classification.

1. Build the catalog’s distinct collection memberships from the successful participating bindings, including a binding that validly emits zero records. Keep stable order as required by existing APIs. Keep the one-to-many file-to-segment route separately. Make add, replacement, rebuild and reconciliation use the same membership invariant. Preserve one scalar membership per applicable file index.
2. On the public managed path, import 768 unique line-framed records into one collection with target 256 and max files 1. Inspect the published catalog row’s `collections` length (one), scoped and unscoped reverse lookup (768), and public before/after record counts. A compatible physical rename followed by `move_file` should report 768. An unchanged update should recognize its unchanged membership and the existing revision/content rules. Replace with two records and check the previous count (768), new count (2) and absence of the other 766. Repeat with two collections and check one membership in each, 768 keys per collection, 1,536 collection-scoped keys overall and an unaffected direct neighbor. Fix the writer of duplicate memberships. Sorting/deduplicating every count result wastes work and leaves the stored invariant broken.

### II.3 · Commit all bindings of one physical file in one publication

**Trace.** In `construction/initial.rs`, jobs are formed inside the loop over collections. A large-file job uses `PrivateSegmentGroup::new(collection_index)` and publishes that group immediately. The multi-segment swap in `lifecycle/publication.rs::publish_initial_segments` is already useful for a single collection. Its input can still contain only the first half of a physical file’s required collection work.

1. From discovery, determine the complete set of eligible collection bindings for the physical catalog file. Use the catalog’s existing path/identity rules. A task unit needs the observed file revision, expected bindings and completion state for each binding. Keep collection-specific compiled schemas, identity policies and representations separate. Two collections may assign different IDs or data types to identical input.
2. Stage all successful bindings privately. Record empty success, fallback and fatal/cancelled outcomes distinctly. A private segment, a finished reader callback or a single complete collection cannot by itself make a file searchable. Form a publishable candidate only after every expected binding completed and all file preconditions still hold. Combine the catalog row, live record slots, file routes, inference observations, sort keys and postings at one installation point.
3. Keep batching for ordinary small files. One practical arrangement is to group small files with the same binding set in a bounded job, stage the relevant collection segments, then publish the completed files together. Handle mixed binding sets with separate bounded groups or explicit completion accounting. A segment with records from several files may be exposed only after the files it would expose are eligible for that publication. Isolate slow large/shared files so an unrelated completed file can still become visible. Avoid one root swap or one segment per small file.
4. Change `prepare_initial_segment` to update a uniquely owned private root or consume and return that root. Its current `next.clone().with_segment_observation(...)` inside the group loop re-shares growing route maps and segment vectors immediately before `Arc::make_mut`. Share with the old pinned root once and accumulate all eligible changes privately. Preserve the current single root swap. Measure preparations, finished segments, groups and actual swaps separately.
5. On fatal error, invalid range, cancellation, duplicate ID or stale observation, stop admitting work for that candidate, join/drain its outstanding jobs and release its transient state. Preserve the previous visible revision and other independently completed groups. Existing successful malformed/raw fallback remains a reader-defined outcome. Initial discovery can expose its established pending state with zero searchable members.

**Acceptance.** Run through public progressive construction with one physical file selected by `entries` and `references`. Pin the catalog view before parsing. Use a test gate to hold the final binding before the file becomes eligible; the gate must work regardless of callback order. With one binding finished, neither collection exposes this file’s records. Another independent complete file can be searchable. On release, one new root first contains the complete expected sets and reverse routes for both collections. Recheck the pinned earlier view. Repeat with a fatal last reader, cancellation after a private segment finalizes and an ID conflict. Inspect the intermediate views, since a correct final count alone passes the broken implementation. Run once below and once above the current 8 MiB initial-build boundary, plus an empty success. Count actual root changes rather than treating the existing `searchable_batches` metric as a publication count.

### II.4 · Use bounded streaming through the registered reader interface

**Trace.** `SourceReader::stream_build_chunks` and `supports_streaming_build` already exist. `prepare_custom_stream` processes each callback synchronously and pushes a complete `BatchContribution` into a vector. The generic large-file dispatcher calls that path to completion before feeding the accumulated vector to admission. It sets peak weight to the configured chunk size and peak active jobs to one. The ordinary small-file initial path can entirely bypass the advertised capability. The built-in large JSON Lines initial build uses a different scheduling path. Its managed mutation helper `prepare_bounded_json_lines` also accumulates a vector of contributions, so port both mutation routes to incremental admission. Assignment 2 succeeds only when a third-party reader gets bounded behavior via its ordinary registration.

**Implementation sequence**

1. Start with the existing pipe-delimited test reader and the worker admission logic in `process_bounded_json_lines_reader`. Extract or adapt the existing bounded scheduling and ordered-result logic for any registered reader that can supply independent chunks. The reader controls framing and where records may be decoded independently. For a stateful reader that cannot split decoding, allow serial emission and incremental core admission. An implementation may reuse the existing callback API. Document what is safe to call concurrently, where borrowed input expires, and how the reader reports a late error.
2. On each emitted chunk, validate its file-relative start against consumed input, checked end arithmetic, byte coverage, ordered nonoverlapping ranges, maximum range count, and absolute file extent. Preserve legitimate gaps between spans for delimiters and existing empty-record semantics. Enforce the configured byte limit with the established single oversized record exception. A reader that emits several records in one over-limit chunk must be rejected or split safely before admission. Return the error through the reader callback and discard that file’s candidate.
3. Assign an ordinal within the file/binding. Queue independently decodable work under the existing global worker and admitted-byte budgets. Transfer owned chunk storage to the worker or borrow it only inside a guaranteed synchronous lifetime. Charge meaningful retained capacities, including the range vector. Account for producer lookahead and retained worker results separately where their sizes differ. Use a bounded ordinal completion buffer. If the oldest outstanding chunk stalls, stop acquiring more input when the window fills. Drain the oldest ready contribution into segment admission before taking more credit. Release input and intermediate contributions promptly after consumption.
4. Separate private segment construction from public visibility. The admission sink may seal private core-owned segments while the next chunk is still unread. The file completion event must wait for EOF, validation, all outstanding worker results and all required collection bindings. Keep compact final segments and necessary routing data. Avoid a full-file `Vec<BatchContribution>`, full-file range list, or second decoded copy retained until EOF. An intentionally whole-document reader can retain a contiguous document buffer.
5. Use the same incremental consumer for managed add/update and fresh builds, including the small-file branch when the reader requests streaming. Reconcile the existing `PreparedFileBuild` consumers that expect a count, logical layout for verified removal, or arguments for rebase. Derive counts as batches arrive. Produce layout evidence only for callers that need it, or derive it from finished segments. Private data needed for a retry remains associated with its observed revision. Hash once during the ordinary input pass for one compatible binding. Count additional decoders when bindings require different representations.
6. Use the existing pool. A coordinator blocked waiting on work running on its own only remaining worker can deadlock. Provide an inline path with one worker, or use the pool’s supported cooperative scheduling. With several independent chunks, permit overlap. Share the job and byte credits across concurrently active files so a limit cannot multiply with the number of callbacks. Independent files must also make progress in parallel with no callback-order promise. Feed results in the order required for stable file offsets, generated IDs and diagnostics. Check cancellation while waiting and release all job and byte permits on every exit.
7. Measure actual open/read calls, bytes, active decoders, staged-but-unadmitted contributions and peak input reservation at the point each operation happens. Report final private segment memory separately. A constant copied from a configuration field or a counter inferred from a digest cannot establish runtime behavior.

**Reader tests**

- Register a non-JSON format through the public registry. Its reader emits complete pipe-delimited records. Count stream callback invocations and ordinary whole-file framing invocations so a test cannot silently run the alternate adapter. Frame `one|München\ntwo|Leipzig` with a deliberately small input buffer. For the existing frame convention the half-open record spans are `[0,12)` and `[13,24)`. Split inside the UTF-8 text at an input-buffer boundary, end without LF, and verify offsets refer to original bytes. Compare logical results with the serial whole-file reference. Preserve the separate JSONL CR/LF convention in the built-in fixture.
- Use test barriers to complete two independent chunks in reverse ordinal order. With several workers show two simultaneous decoders for the registered format. With one worker prove completion without a timeout or busy loop. Register two distinct files with barriers and show their callbacks can overlap without assuming which file starts first.
- On a large stream, block input **before EOF, after enough complete records to seal a segment**. Assert a private segment actually finalized, intermediate memory is bounded, and no record from the new revision is searchable. Resume, verify all records and exact final span, then repeat with invalid ranges, a fatal reader and cancellation near the end. On update the previous version remains fully queryable.
- Repeat for one single record larger than the normal chunk budget, and for many small records while the oldest decode is deliberately stalled. Expect the documented single-record allowance or a defined resource result. Demonstrate bounded completion buffering and no starvation. The real peak may exceed the normal chunk size in the oversized case and must be reported accurately.
- Retain the built-in 100,000-record scale fixture as a separate comparison. It contains 30,000,000 bytes. With a 4,096-record target and max files 1, expect 24 full segments plus one containing 1,696 records on the unchanged fixture. Expect exactly one live file, 100,000 live records and final range `[29999700,29999999)`. Record content opens, bytes, chunks, segments and file-revision installations independently. Run the registered reader at comparable scale so the specialized reader cannot stand in for it.

### II.5 · Make file metadata restrictions selective after additions, replacements and deletions

**Trace.** Inspect `inventory/scalar.rs`, `lifecycle/catalog.rs::{appended,replaced,retired,scalar_ids,cached_scalar_ids}`, `files/mod.rs` and the size/time restrictions in `query_execution/restrictions.rs`. On retirement the mutable size/time memberships survive. The current limit can be exhausted by obsolete base/delta candidates before liveness or deduplication is applied. A cache hit still clones an owned vector for per-segment filtering. Repeating the lookup for each segment defeats routing when one file matches.

1. Maintain one current size, modification-time and known/unknown creation-time membership per live file. In a delta replace, remove the old contribution and insert only changed values. In retirement, remove the delta contribution and mask any underlying base value. In a path-only move, share scalar storage when the values are unchanged. Preserve immutable older roots for pinned queries. Keep native integer sizes and the engine’s exact comparison semantics for fractional query literals.
2. Resolve the immutable base plus mutable delta into distinct live `FileId` candidates. A changed base file must not appear through its old key. Apply the selectivity cap to distinct live candidates. Physically remove obsolete mutable entries. If a base equal-value range can be dominated by tombstones, use the existing live/override structures or an affected-range mask to skip those base members efficiently. Sorting/deduplicating after the raw capped read has already returned “too broad” cannot fix the error. Cache entries must belong to the same view and comparison semantics, including any limit that affects the answer.
3. Resolve a supported size, modified, created or path restriction once for a pinned query. Reuse that result across segment evaluation. For a selective result, use the existing base file-record directory and `RecordRoutes::segments_for_file` in each relevant collection. Intersect returned slots with the segment’s live mask and incoming domain. Mark an untouched segment’s positive leaf empty without probing every file candidate there. Do this where the query can share the work across segments, rather than at each segment’s leaf callback. Use the existing wide traversal when cheaper for a genuinely broad predicate.
4. A file-only leaf in an OR is only one arm of the expression. A direct record satisfying the other arm must survive. Preserve exact missing/null semantics, supported date/time conversions, negation, evidence, sorting and pagination. Test the live catalog as well as query execution. Fileless `@file.created` is missing. An imported file with unavailable creation time has explicit null. Pure metadata evaluation and locator-only counts must use zero file or provider content reads.

**Churn fixture.** Build many unrelated live segments. Add 4,097 files of the same size through the file catalog. Retire 4,096, leaving one live. Repeat with all 4,097 retired plus one newly added survivor, crossing the current raw limit in both variants. Include one empty-file catalog row and one direct record with an ordinary `path` field. Pin views before and after retirement. Exercise exact size equality, a fractional size bound, tied timestamps, a moved file, a changed size, unknown creation time, an OR that selects the direct record, negation and a broad range. Compare optimized results with exact evaluation and compare public catalog lookup with live file and record identities. Use injected creation-time observations in a focused test if the filesystem always supplies that field.

**Acceptance counts.** For one selective comparison in a query, observe one resolved restriction, the expected distinct live file IDs, membership-list probes only for participating segments and zero content reads. A matching file spanning several segments legitimately causes several probes. Track fallback selection and the reason. A growing stream of retired delta keys, or unrelated segment membership probes increasing with the repository size, fails this test. Counts from point lookup by record ID do not measure the work of a metadata predicate.

## Integrated verification and handoff

### Public mixed-collection acceptance test

Use one public end-to-end test as the final integration check. Register a managed reader plus a direct registration with a versioned provider for collection `entries`. Establish a rootless direct-only test separately to ensure the optional file service stays optional.

1. Submit retained `manual-1`, import a two-record file with intrinsic IDs `file-1` and `file-2`, and submit versioned `manual-2`. Give all four records a searchable common title. Both FTS and an ordinary field filter return all four. An exact file-path filter and reverse file lookup return the record keys `(entries, file-1)` and `(entries, file-2)` under the single catalog FileId. Each direct key returns absent file info. Check that querying an unknown key is distinguishable from finding a known direct record with no file.
2. Pin that view. In a second revision replace the file with a one-record file, then perform a compatible move and deletion. At each transition verify IDs, record count, old/new path, reverse membership, locator and the result of `@file.size > 0 || @record.id = 'manual-1'` using the repository parser’s accepted syntax. Initially the OR selects three records. After replacing with one positive-size file record it selects two. After deletion it selects only `manual-1`. Unfiltered metadata sorts still contain both direct records in their normal missing-value order.
3. Check retained logical payload and `OriginalBytesUnavailable`, provider-versioned projected reads, and file-backed original range reads. Query the old pinned view after replacement and inspect the allowed payload outcome. Exercise the same fixture in file-first order. Run compaction, reconciliation and supported rebuild coverage. Confirm unaffected direct records and their owner/version survive.
4. Run the concurrency interleavings separately with gates. Queries during a paused file update see the prior file in full with an independently published direct change. Queries during a held initial shared-file group see either pending/previous file membership or the complete new revision. Check no partially published state even when final counts agree.

### Commands and evidence

Use the package names and examples below from the task worktree. The first commands give fast feedback. Add the new regression filters as they are implemented. If a filter matches zero tests, correct the command and capture the actual test count.

```bash
cargo test --locked -p objekte-search-core --test record_ingestion
cargo test --locked -p objekte-search-core --lib failed_middle
cargo test --locked -p objekte-search-core --lib registered_streaming_reader
cargo test --locked --release -p objekte-search-core --lib \
  spool_overflow_accepts_beyond_resident_descriptor_window -- --ignored
cargo run --locked -p objekte-search-core --example record_ingestion_public
```

For the final working tree, run:

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo build --locked --release -p objekte-search-core --features detailed-profiling \
  --example bench_native_record_ingestion --example bench_managed_file_scale \
  --example bench_build_musenalm
```

Use unique disposable fixture directories. The scale and routing examples create and remove their fixture roots. Run from the repository root. The following invocations assume Cargo’s default target directory. Substitute the resolved release examples directory if the local build overrides it.

```bash
measurement_root="$(mktemp -d)"
mkdir -p "$measurement_root/spool-singletons" "$measurement_root/spool-batches" "$measurement_root/spool-overflow"
target/release/examples/bench_native_record_ingestion ingest 1 1 normal 65536 "$measurement_root/spool-singletons"
target/release/examples/bench_native_record_ingestion ingest 256 1 normal 65536 "$measurement_root/spool-batches"
target/release/examples/bench_native_record_ingestion ingest 256 4 overflow 65536 "$measurement_root/spool-overflow"
target/release/examples/bench_native_record_ingestion routing 64 65536 "$measurement_root/routing"
target/release/examples/bench_managed_file_scale "$measurement_root/large-jsonl"
```

Run the existing many-file Musenalm build with `bench_build_musenalm --root <actual fixture> --alphabet ascii --build-workers 32` and repeat for `unicode` on the normal 32-worker host. Use the same host, compiler, build features, dataset and worker settings for baseline and final. Give baseline and final runs fresh fixture paths. The existing comparison runner under `docs/ai/generated/core-managed-file-ingestion/` documents fixture preparation and options. Extend a targeted test or benchmark for the **registered** streaming reader, with explicitly set one-worker and multiworker configurations. The unmodified JSONL example alone cannot supply that comparison.

| Workload | Minimum evidence to save | Interpretation |
|---|---|---|
| Direct singleton and batched publication | Records, producer count, producer acceptance, publish drain, installed roots, initial segments, peak RSS and resident/in-flight input | Earlier repair recorded about 89 ms drain for 65,536 accepted singletons, down from 30.367 s. The Assignment 2 comparison measured about 92 ms for the same drain. Compare identical timing boundaries and explain root counts. |
| Forced spill and receipt lifetime | Waiting descriptor maximum, active descriptors, spool bytes held/reclaimed, retained completion blocks or spans, allocation/copy evidence | The resident queue cap holds even as outstanding work crosses 65,536. Completed historical receipts do not accumulate in the session once their owners release them. |
| Ordinary JSON and conditional update | Open counts, content bytes, handle/path observations, identity fallback, record IDs/ranges and final publication result | One successful input pass for one reader on the standard path. Metadata verification and genuine retries have separately identified costs. |
| One large file, two reader implementations | Worker count, simultaneous decoders, queued/admitted byte high-water, private segments before EOF, final segments, publication count, read bytes, RSS | Specialized historical fixture recorded 391 segments at 375.6 ms for 100,000 records. Demonstrate correct segment formation, generic streaming progress and actual input accounting. |
| Many ordinary files | Build time, records/s, RSS and observed opens/bytes on the same fixture | Inspect a material throughput loss for duplicate I/O, per-file segment/root creation, serialized callbacks or repeated route copying. |
| Metadata after churn | Query result and reference result, matched files, membership-list visits, fallback decisions, reads, old and current view | Show locality with increasing unrelated segments and retired files. Explain broad-predicate work separately. |

Use short paired runs to check differences large enough to affect a workload. Repeat noisy or materially regressed cases before concluding. Differences of a few microseconds in add or move have **no acceptance threshold**. A byte-for-byte wrong result, a false conflict cancelling accepted records, a partial revision, an avoidable full-file pass or unbounded intermediate growth requires correction independently of its timing.

### Common diagnostic signatures

| Symptom | Check first |
|---|---|
| Concurrent file succeeds, direct receipt reports a revision conflict | Confirm a retry owns the original prepared content and checks touched keys against the latest root. Verify final placement does not consume the only copy on a failed swap. |
| The failed middle batch’s first record appears | Look for per-record publication or a prefix cut at a record boundary inside a member. Exclude the entire failed member in every index and route. |
| Spill tests pass but RSS grows as paused backlog increases | Count both descriptor containers, chunk metadata, completion history and live caller-held receipts separately. Test with most receipt handles dropped. |
| ID search finds B, but original bytes or catalog identity describe A | Trace which open supplied metadata, which supplied bytes and when the pathname was verified. Inspect default framing, malformed fallback, conditional and initial-build routes. |
| Shared-file final count is correct but an intermediate query sees only one collection | Look for per-collection root swaps or segment publication before the physical file’s complete binding set is ready. |
| Three segments in one collection produce a threefold mutation count | Inspect the catalog row immediately after add. Record distinct collection IDs at membership formation. |
| Every JSONL chunk becomes a segment | Inspect the state immediately after the first chunk. Ask whether the next chunk introduces any distinct file into that segment. |
| Reader advertises streaming but the probe reports one active job or fixed peak bytes | Check the actual registered entry point, small/large dispatch, pre-EOF private admission, real job counters and capacity measurements. |
| Query reports zero content reads but scans every segment | Count actual membership probes and the point where the file candidate restriction was produced. Zero I/O alone does not prove selectivity. |

### Delivery decision

Push the implementation and report commits to the integration branch. Record the actual base, final production SHA, report SHA in the handoff, compiler/host, commands, raw evidence paths, time and memory units, and exact test outcomes. For each checkpoint A–G include a short row with the baseline failure, implementing commit, test name and final result. Record environment-unavailable tests as unverified with a specific reason. Follow the repository workflow for the completed task document. Keep only the compact requested evidence in the generated directory.

The implementation is ready for review when both ingestion routes work together under the stated races, the resource and membership invariants are measured on the production paths, and the final branch can reproduce the results. Deliver the integration branch to the maintainer for the merge decision.

### Source references

- [Reviewed Assignment 2 implementation and completion report](https://github.com/Simon-Martens/objekte/blob/f5b846771ccf63151ad33f07739f5a19dcb26505/docs/ai/done/objekte_core_managed_file_ingestion_specification.md)
- [Earlier direct-ingestion repair decision and measurements](https://github.com/Simon-Martens/objekte/blob/3bf0811943b11837bf997f51ee9f3392e0670e99/docs/ai/generated/record-ingestion-and-file-catalog/repair-summary-732725bcfb.md)
- [Assignment 2 paired benchmark report](https://github.com/Simon-Martens/objekte/blob/f5b846771ccf63151ad33f07739f5a19dcb26505/docs/ai/generated/core-managed-file-ingestion/benchmark-summary-bf34c076e1.md)
- [Large-file raw sample](https://github.com/Simon-Martens/objekte/blob/f5b846771ccf63151ad33f07739f5a19dcb26505/docs/ai/generated/core-managed-file-ingestion/scale-100k-bf34c076e1.json)


## Implementation report

Completed 2026-11-25 on `integration/ingestion-publication-and-resource-repairs` in `/home/simon/source/objekte-integration-ingestion-publication-and-resource-repairs`. The fetched base was `f5b846771ccf63151ad33f07739f5a19dcb26505`, which contains the earlier repair at `3bf0811943b11837bf997f51ee9f3392e0670e99`. Final production code is `038051d8d1f8bdc6b93daafe3a1c57ef8e06a7bd`. Validation and release measurements used `2b97e38698777bbaea85b8e64c7693ecdcdfa78c`, which adds test fixture adjustments only. No subordinate worktrees were created.

| Checkpoint | Baseline defect and repair | Regression evidence on the final tree |
| --- | --- | --- |
| A | An unrelated commit could reject a prepared batch with an index revision conflict. Prepared indexes now survive root placement, which validates the live registration and touched keys. File and direct placement share coordination after source reading. `ca5885ef01`, `aa6213b92b`. | `prepared_direct_batches_survive_unrelated_file_publication` passes grouped and ungrouped schedules, both conflict outcomes, and a two-operation losing group member. Restoring the old revision check reproduced the false conflict. `direct_batch_publishes_while_multisegment_file_update_is_paused` covers the reverse publication order. |
| B | Grouping cloned operation bodies, the overflow deque grew past the resident limit, and early receipts retained the session history. Groups now take ownership of payloads, recover complete valid prefixes, and store the spilled suffix in versioned spool frames. Completed receipt blocks leave the ledger. `4bc36515e7`, `08a6469719`, `8eccc74f29`, `b460540098`. | The retained string allocation survives publication in `coalesced_retained_payload_keeps_its_owned_string`. Both `failed_middle` tests and `receipt_ledger_releases_old_completed_blocks` pass. Release spool stress accepts 100,000 then 120,000 paused submissions, followed by four producers during drain. The final [recorded run](../generated/ingestion-publication-and-resource-repairs/spool-stress.txt) completed 121,024 receipts with a peak of 65,536 resident waiting descriptors, separately accounted active work, and zero pending spool bytes at finish. |
| C | Discovery evidence could label another file's bytes and managed preparation could reopen its pathname. Initial discovery mismatches now abort, managed framing uses the observed handle, byte conditions compare during decoding input, and rebuild rechecks each observed file at handoff. `61c4dcf4fb`, `aa6213b92b`. | `fresh_build_rejects_same_stamp_replacement_after_discovery`, `observed_handle_supplies_whole_document_while_path_is_replaced`, `supplied_bytes_update_preserves_source_and_checks_deletion_conflict`, `programmatic_rebuild_rejects_source_replacement_at_handoff`, and `pinned_file_bytes_reject_same_stamp_replacement` pass. |
| D | A continuation could seal at the distinct-file limit and a file could accumulate one catalog membership per segment. Both builders now respect distinct-file limits and split streams at record boundaries. The catalog records each participating binding once. `9510243be9`, `baaae794a2`, `038051d8d1`. | `small_jsonl_partitions_follow_the_record_target_across_file_continuations` verifies 4, 4, 2 and a second file. `ordinary_file_batches_respect_distinct_file_limit_before_decoding` verifies two files then one. `multisegment_managed_file_has_one_membership_per_collection` verifies 768 keys per binding, 1,536 total, and exact move and replacement counts. Legacy inference and query fixtures now explicitly request the shapes their assertions require. |
| E | Initial per-collection jobs could expose a shared file before all its bindings were ready. Shared files now stage all participating bindings and install them in one root. Root preparation accumulates segments without cloning the growing root per segment. `867d410b38`, `b460540098`. | `shared_initial_file_is_hidden_until_all_bindings_are_complete` pauses the final binding below and above 8 MiB. The independent file stays searchable. Success, empty input and a fatal final binding preserve the expected pinned and current views. |
| F | Registered framing could be bypassed for small files or retain every intermediate contribution until EOF. Registered and built-in streams now feed ordered admission into private segments. Managed streams share job and byte credits, and small independent registered files can overlap in the build pool. `1dbc7a2f50`, `aa6213b92b`, `baaae794a2`. | `registered_streaming_reader_initial_build_dispatches_small_files_and_decodes_in_order` checks one worker, reversed completion with two workers, exact UTF-8 byte spans and one content open. `registered_streaming_reader_callbacks_overlap_across_small_files` proves independent callback overlap. `registered_stream_seals_private_work_before_eof_and_discards_late_failure` confirms a real segment finishes while the source is held before EOF and a late error publishes none of it. |
| G | Retired candidates exhausted the cap, and per-segment metadata filtering repeated membership work. The file scalar indexes mask obsolete base memberships and remove retired delta entries. Pinned query restrictions project only through matching published segment routes. `26e2443f60`, `aa6213b92b`, `b460540098`. | `scalar_cap_counts_live_members_after_base_and_delta_churn` crosses the 4,096 cap for both storage shapes. The public release churn fixture agrees with reference results after retiring 4,096 of 4,097 files, then replacing the last survivor. Consolidation initially exposed a payload/published ID mismatch, reproduced as a zero instead of one match. The corrected [release run](../generated/ingestion-publication-and-resource-repairs/churn-stress.txt) resolves once, matches one file, probes one membership list among 63 or 64 segments, and reads zero content. |

### Integrated verification

`cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace`, and `cargo run --locked -p objekte-search-core --example record_ingestion_public` passed. The [workspace test output](../generated/ingestion-publication-and-resource-repairs/workspace-tests.txt) includes 718 passing core unit tests and 26 passing record-ingestion integration tests. The latter includes the public end-to-end `one_collection_interleaves_managed_files_retained_and_versioned_records` in both ingestion orders. The release stress commands below passed separately.

```bash
cargo test --locked --release -p objekte-search-core --lib spool_overflow_accepts_beyond_resident_descriptor_window -- --ignored --nocapture
cargo test --locked --release -p objekte-search-core --test record_ingestion routed_size_filter_stays_selective_after_managed_catalog_churn -- --ignored --nocapture
```

### Benchmarks

Linux-7.3.3-200.fc44.x86_64-x86_64-with-glibc2.43, 32 available CPUs, performance governor, system allocator, rustc 1.99.1 (58cf8f9c3 2026-10-13), and cargo 1.99.1 (5f94df478 2026-08-27). The [paired runner](../generated/ingestion-publication-and-resource-repairs/compare.py) built both revisions with `cargo build --locked --release -p objekte-search-core --features detailed-profiling`. It used the same Musenalm fixture and 32 requested build workers for ASCII and Unicode. The [manifest](../generated/ingestion-publication-and-resource-repairs/comparison-2b97e38698.json) identifies the binaries and the [62 observations](../generated/ingestion-publication-and-resource-repairs/comparison-2b97e38698.jsonl) preserve invocation arguments and raw results. The tables use three alternating baseline/final pairs per workload, except routing, which uses one pair.

| Construction workload | Baseline wall / peak RSS | Final wall / peak RSS | Production work |
| --- | ---: | ---: | --- |
| Musenalm ASCII, 148,486 records | 704.805 ms / 655.0 MiB | 690.841 ms / 639.8 MiB | 210,677 → 214,935 records/s. Both opened 148,486 sources and read 193,297,230 bytes. |
| Musenalm Unicode, 148,486 records | 739.923 ms / 655.6 MiB | 777.379 ms / 661.1 MiB | 200,678 → 191,009 records/s. Both opened 148,486 sources and read 193,297,230 bytes. |
| JSONL, 32 workers, 100,000 records | 360.841 ms / 76.6 MiB | 63.382 ms / 63.1 MiB | 391 → 25 segments, median peak decoders 16 → 16, one file publication in each run. |
| registered, 1 worker, 100,000 records | 541.717 ms / 75.3 MiB | 287.667 ms / 54.4 MiB | 391 → 25 segments, median peak decoders 1 → 1, one file publication in each run. |
| registered, 8 workers, 100,000 records | 527.483 ms / 75.9 MiB | 95.007 ms / 56.3 MiB | 391 → 25 segments, median peak decoders 1 → 6, one file publication in each run. |

Both scale formats read exactly 30,000,000 bytes with one content open and ended at `[29999700,29999999)`. Each final scale run produced 24 full segments and one with 1,696 records. The JSONL and registered pipe data have equivalent field shapes and independent framing implementations. Counters distinguish completed decoder jobs, reserved input and finished index memory.

| Direct ingestion of 65,536 records | Baseline acceptance / drain | Final acceptance / drain | Baseline → final peak RSS |
| --- | ---: | ---: | ---: |
| Singletons, one producer | 15.443 / 82.335 ms | 16.531 / 68.563 ms | 225.1 → 156.9 MiB |
| Batches of 256, one producer | 0.335 / 189.798 ms | 0.334 / 194.582 ms | 121.8 → 123.0 MiB |
| Forced spool, batches of 256, four producers | 31.768 / 103.886 ms | 15.070 / 103.705 ms | 186.4 → 185.4 MiB |

Drain measures the wait after acceptance. 1-normal pairs measured 795,971 → 955,852 records/s over the drain interval and 18 → 17 root installations. 256-normal pairs measured 345,293 → 336,805 records/s over the drain interval and 256 → 256 root installations. 256-overflow pairs measured 630,848 → 631,944 records/s over the drain interval and 70 → 70 root installations.

The shared 64-file metadata probe returned 256 matching records and used zero content reads. Cold query medians were 187.543 → 127.162 µs and 500 warm queries took 67.595 → 43.525 ms. The mixed routing probe returned 1,024 matches for the size filter and 1,025 for the OR with a direct record. The selective size filter resolved once, probed one file membership list and read zero content. The broad churn case used the wide metadata traversal only while the comparison matched more than 4,096 live files.

Small lifecycle medians, baseline → final in milliseconds. Build 4.456 → 4.858, add 1.602 → 0.616, append update 0.213 → 0.640, verified update 0.205 → 0.332, move 0.036 → 0.044, delete 0.034 → 0.068, reconciliation 0.120 → 0.127.

The initial performance audit found an avoidable pathname stat after each ordinary read and serial metadata verification at handoff. The final code checks the observed handle during extraction, then verifies the path once at handoff and uses the worker pool for large partitions. The benchmark verifies equal content opens and bytes on the many-file fixture. Timing differences of a few microseconds are reported as measurements.

### Limits and delivery

The verified identity tests ran on Linux with device and inode evidence. Platforms without reliable identity need a verified digest comparison or receive an explicit unverifiable result. An external writer that preserves the same object identity, length and modification time can leave those observed markers unchanged. Independent callback overlap was measured on small registered files and decoder overlap on one large registered file. The two shapes are reported separately.

The integration worktree remains available for the maintainer's merge decision. The authoritative task text above is preserved byte for byte.
