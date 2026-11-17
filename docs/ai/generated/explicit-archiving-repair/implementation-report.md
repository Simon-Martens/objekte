# Explicit archiving repair implementation report

Task: `explicit-archiving-repair`

Base: `29613ab1ccbcd9c01b2b44de3288ad6dab044736`

Implementation: `bfc1f0a1c2`

## Outcome

The repair specification's correctness work was implemented and verified. The
current catalog is now included in inventory/no-op/accounting traversal,
quiescence no longer holds the transition gate while joining workers,
component conversion is prepared non-destructively, record identity maps retain
their original ownership, segment and catalog row serialization is direct,
segment conversions are independently scheduled, cold metadata execution uses
borrowed rows, and benchmark phases/provenance are separated.

## Repair map

| Repair | Result |
| --- | --- |
| R01 | Current and retained base catalogs are enumerated separately; alias owners are deduplicated; delete-to-archive converts native current-catalog shards and repeated no-op preserves generation. |
| R02 | Explicit Running/Draining/Quiesced close coordination; the transition gate is released before worker joins; concurrent callers share completion; resume during draining is rejected. |
| R03 | Posting dictionaries, segment rows, join fields, and candidate roots prepare locals before assignment. Operation-scoped fault controls work in workers at serialization, byte-check, semantic, final-candidate, and pre-install stages. |
| R04 | Inventory rows, fingerprints, identity order, relation arrays, and path wrappers use shared ownership; archive adopts an archived base path directory and releases direct path maps. |
| R05 | `RecordLookup::native` moves the original `HashMap<RecordId, RecordSlot>`; archiving serializes its borrowed iterator and lookup uses `Borrow<str>`. |
| R06 | Segment rows serialize through borrowed file/record/diagnostic/vector adapters without staging owned row vectors. Catalog iterators are direct; the remaining owned catalog-row adapter is noted below. |
| R07 | Segment jobs are deduplicated and flattened across groups; the pool is capped to useful jobs; a deterministic one-collection/many-segment test observes multiple active jobs. Base/catalog conversion remains the serial prefix. |
| R08 | Archived ID validation uses borrowed strings, direct lookup probes, and one ownership-visitation bitmap. Existing checked byte and semantic checks remain. |
| R09 | Q001 and Q119 cold paths use borrowed row/scalar access. Cold zero-result metadata execution initializes no whole compatibility cache. Q001 improved from roughly 51-57 ms to roughly 8-9 ms; Q119 is roughly 6 ms. |
| R10 | Reports expose logical and owner counts, component conversion, installation, retirement, serializer/validation sums, bytes/capacity, residual payloads, workers, and peak jobs. Benchmarks capture native and archive timings separately and embed executable revision/digest. |
| R11 | A01-A42 evidence is mapped in `test-coverage.tsv`; new tests cover catalog-only archive, failure stages, deadlocks, replacement, borrowed rows, cache-free metadata execution, and bounded segment concurrency. |

## Validation

- Core library suite: 600 passed, 17 ignored, 0 failed.
- Workspace all-target check: passed.
- Detailed-profiling all-target check: passed.
- Compaction API test: passed.
- Full workspace tests: one existing private-drain test exceeded its 120-second
  deadline under default parallel full-workspace load; it passes alone and in
  the core suite. The serial workspace rerun is recorded in the completed task.
- Explicit archive example: native/archived/mixed/final counts `1/1/2/2`.
- Clippy and final workspace checks are recorded in the completed task.

## Benchmarks

Direct query aggregate was 1.070855x native overall; progressive was
1.033761x native overall. All 300 alphabet/case result digests match.

Median native build was 764.760 ms ASCII and 823.043 ms Unicode. Median
explicit archive was 152.944/153.714 ms at one worker and
111.499/108.627 ms at 32 workers. Archive output was 149,152,416 serialized
bytes and 233,957,056 aligned capacity bytes.

## Known limitations

- Catalog metadata serialization still constructs one owned `StoredFileRow`
  value at a time because the fully borrowed resolver failed checked archive
  validation. It does not stage a vector, but it is not yet allocation-free.
- Base collection and catalog serialization still precede the parallel segment
  schedule rather than participating in the same job queue.
- Lifecycle mutation timings were not separated into a dedicated benchmark run;
  their correctness is covered by the lifecycle/model tests and evidence files.
- The benchmark matrix uses one query repetition per case and three construction
  repetitions, the user-authorized smaller protocol.
