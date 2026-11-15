# RocksDB bulk construction repair report

Date: 2026-11-15  
Task: `rocksdb-bulk-construction-repair`  
Actual parent: `00d16a040bc5f3bf83daba8ef6b0daa9657b9f0c`  
Historical first repair: `08591e8452132f392201cfdb745d958f502a556b`  
Native control: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Integration branch: `integration/rocksdb-bulk-construction-repair`

## Decision

**Performance failure.** The complete-image constructor removes the specified
repeated bitmap reads and reduces caller construction from the first repair's
35.650 s to 11.585 s, a 3.08 times improvement. It remains 11.69 times slower
than the matched native build and exceeds both memory gates.

| Observation | Native | Bulk W=1 | Bulk W=4 |
| --- | ---: | ---: | ---: |
| Caller construction | 990,894,820 ns | 11,584,790,610 ns | 11,669,616,128 ns |
| Resident RSS | 399,290,368 B | 1,097,539,584 B | 1,103,892,480 B |
| Peak RSS | 410,054,656 B | 1,191,686,144 B | 1,196,048,384 B |
| Time ratio | 1.00 | 11.69 | 11.78 |
| Resident ratio | 1.00 | 2.75 | 2.76 |
| Peak ratio | 1.00 | 2.91 | 2.92 |

The unchanged gates required at most 1,139,529,043 ns, 559,006,515 resident
bytes and 656,087,449 peak bytes. All fail. This is not a claim that RocksDB
cannot be made faster; it is a measured failure of the tested
TransactionDB/one-column-family design.

W=1 was 0.7% faster than W=4. Publication is coordinator-serialized so that
one authority owns each live bitmap tail. W=4 is recorded and accepted by the
worker-policy code, but does not claim parallel publication in this
implementation. Disjoint writer overlap is therefore not evidenced.

## Implemented mechanism

- Discovery now returns an immutable inventory directly. It writes only seven
  collection metadata rows, not per-file `Files`, `Paths`, `QueryPaths`,
  `PendingFiles` and build-state rows that would immediately be read and
  deleted.
- Preparation chunks remain bounded at 256 files / 8 MiB. Dispatch is bounded
  by `min(16, 2P + W)`, and a real estimated-byte check prevents new dispatch
  when the 64 MiB target would be crossed. A completion may always publish;
  actual measured chunk capacity then controls further dispatch.
- Normal construction forms cohorts at the anchor collection's next 4,096-slot
  boundary, with file, 32 MiB source, 100 ms age and end-of-input seals. The
  first physical file is published alone.
- Typed bitmap identities accumulate sparse offsets in owned block images.
  Complete images are encoded only for dirty keys. An acknowledged unfinished
  tail is retained and reused by the next cohort rather than reread from
  RocksDB.
- Forward postings no longer allocate encoded database keys per gram
  membership. A physical key is encoded once for each distinct dirty bitmap
  image in a cohort.
- Each cohort is one atomic `WriteBatchWithTransaction<true>` submitted through
  `TransactionDB::write_opt`. It contains all file metadata, paths, records,
  domains, complete bitmap images, reverse contributions/grams, sort rows,
  relation rows and one receipt.
- Receipts use a version-4 streamed digest over file identities, versions,
  slot ranges, coverage and the ordered encoded plan. Exact replay is a no-op;
  changed content is rejected.
- Finalization streams value, length, state and FTS groups directly to the
  bounded metadata writer. It retains no whole-directory maps, performs no
  contribution decompression, and merges file inventory with build state in
  ordered iterators.
- Coverage completeness is reduced with logical AND from receipt summaries.
  Empty configured collections start complete from the runtime schema, and
  named custom coverage no longer overwrites a declared field descriptor.
- Validation and candidate reference export consume actual `SortEntries`.
  Missing, extra or altered forward rows fail even when reverse contributions
  are unchanged.

## Operation evidence

The W=1 run processed 148,486 files and records, 74,016,400 bitmap
memberships and produced 415,015 dirty cohort bitmap keys. It performed:

- 0 bitmap database reads.
- 0 cold-tail reloads.
- 415,015 bitmap puts.
- 415,015 physical bitmap-key encodes.
- 2,352,331 static actions.
- 2,767,346 total actions.
- 40 cohorts.
- 31,227 peak retained tail keys / 23,864,904 peak tail bytes.
- 80,200,569 ns to the first-file committed measurement.

The reported put count is cohort puts. It is not the final unique bitmap-key
count, because a first-file key can be written once partially and once in its
completed block. The full validator counted the persisted structure separately.

Seal reasons were 34 file limits, 3 anchor boundaries, 2 accumulation ages,
1 first-file publication and 1 end-of-input seal. No ordinary cohort exceeded
its file or source-byte seal limits.

## Cost attribution

W=1 stage timers:

- Discovery: 823,682,351 ns.
- Preparation wall interval: 10,190,017,932 ns.
- Summed preparation workers: 6,220,214,620 ns.
- Coordinator mapping and accumulation interval: 9,904,280,693 ns.
- Complete-image encoding: 291,247,919 ns.
- Plan digest: 306,679,322 ns.
- Atomic database writes: 2,156,538,931 ns.
- Finalization: 568,083,783 ns.

The preparation wall and mapping intervals overlap and must not be added.
Process accounting recorded 1,902 user and 96 system CPU ticks. At the local
100 Hz clock this is 19.98 CPU-seconds, or about 172% CPU on average.

Sampling profiling was attempted with `perf record -F 99 -g`. It could not
start because `perf_event_paranoid=2` and the host SELinux policy denied CPU
PMU access. Stage timers and process accounting remain valid; no flamegraph or
kernel/native CPU split is claimed.

The pinned `TransactionDB<SingleThreaded>` does not expose the binding's
property APIs, which are implemented only for `MultiThreaded`. Memtable,
block-cache, table-reader, SST and compaction properties are therefore
recorded as unavailable, not zero.

## Memory ownership

The scheduler observes real retained capacities for prepared chunks and
separately reports live tail images and action-plan bytes. Peak prepared
ownership was 67,072,840 bytes at 16 in-flight jobs. Peak retained tails were
23,864,904 bytes. The action plan for W=1 summed to 345,648,981 encoded bytes
over the campaign; the largest cohort is bounded by the seal rules.

These are application-owned estimates. Native batch storage, RocksDB
memtables, virtual SST files, allocator retention and free-list behavior are
not exactly observable through this ledger. The 1.192 GB process HWM includes
all of those and remains substantially larger than application ownership.

## Correctness

The final full ASCII validation passed with zero errors:

- 148,486 files and records.
- 40 receipts accounting for every file.
- 2,814,411 value memberships.
- 921,173 length memberships.
- 14,004,908 state memberships.
- 56,275,908 gram memberships.
- 566,768 actual forward sort entries.
- 152,544 value-directory and 641 length-directory rows.
- 418 coverage rows.

Fixture tests cover overlapping collections, empty and malformed files,
16,384 one-record files sharing common grams, exact scalar ordering, namespace
bounds including all-FF prefixes, forward-sort corruption, Unicode file-mode
build/reopen, memory-host isolation and format/configuration checks.

## Requirement matrix

| Requirement | Status | Evidence |
| --- | --- | --- |
| Direct discovery inventory, no pending write/read round trip | Implemented/tested | Seven discovery metadata puts; direct inventory used by preparation. |
| Complete bitmap images, no normal-path bitmap reads | Implemented/tested | 0 reads and 0 cold reloads in full runs and common-key fixture. |
| One encoded physical key per dirty cohort image | Implemented/measured | 415,015 puts and 415,015 bitmap-key encodes. |
| First-file publication | Implemented/measured | One-file cohort; 80.2 ms to first commit measurement. |
| Anchor-block/file/source/age/end cohort seals | Implemented/measured | Counts and reasons recorded in raw JSON. |
| Whole physical file atomic publication | Implemented/tested | Multi-collection fixture and full receipt validation. |
| One receipt and exact replay digest | Implemented/tested | Version-4 receipt; full receipt/file accounting. |
| Bounded preparation ownership | Implemented/tested | Target-sensitive dispatch test and recorded peaks. |
| Streaming finalization without reverse decode or whole directory maps | Implemented/tested | Grouped namespace scans; 0 contribution decodes in readiness code. |
| Actual forward-sort validation/reference | Implemented/tested | Set equality plus corruption tests; reference scans `SortEntries`. |
| Namespace-clamped iterators | Implemented/tested | None/one-sided/prefix/all-FF tests. |
| Coverage AND, empty collection and named collision handling | Implemented/tested | Receipt summaries; declared-only field row write. |
| Memory-mode host emptiness | Implemented/measured | `memory_host_files: []`. |
| Unicode and durable file reopen | Implemented/tested | Unicode fixture built, flushed, reopened and validated. |
| Writer-count parallelism | Not evidenced | Publication is coordinator-serialized; W=4 is not faster. |
| Native/application/engine CPU attribution | Partial | Process CPU measured; sampled profile blocked by host policy; engine properties unavailable. |
| Feasibility gates | Failed | 11.69x time, 2.75x resident RSS, 2.91x peak RSS. |

## Remaining work

The dominant measured interval is preparation/accumulation. Almost all
74,016,400 memberships still pass through the source fact representation and
owned sparse offset vectors before 415,015 images are encoded. A future design
would need packed typed accumulator output from preparation, disjoint cohort
publication with true asynchronous acknowledgement/ownership, and a lower-copy
native batch assembly path. Such work is outside this bounded repair cycle and
must be separately authorized.

Raw evidence and hashes are listed in `comparison.json`.
