# RocksDB admission repair report

Date: 2026-11-15  
Task: `rocksdb-admission-repair`  
Integration branch: `integration/rocksdb-admission-repair`  
Parent integration: `integration/rocksdb-construction-repair`

## Outcome

The admission task's parent corrections were retained, but its remaining
transactional bitmap read/modify/write design was explicitly superseded by the
later `rocksdb-bulk-construction-repair` specification. The final combined
branch therefore does not rebuild and benchmark the superseded transaction-RMW
algorithm repeatedly.

The combined constructor improved caller construction from the first repair's
35,650,433,616 ns to 11,584,790,610 ns, a 3.08 times improvement. It still
failed the original gates at 11.69 times native time, 2.75 times native
resident RSS and 2.91 times native peak RSS.

## Retained parent work

- `54d07604ac` replaced the abandoned per-file offset-tree path with
  batch-wide block deltas and worker-owned contribution scratch.
- `bc8696eb48` clamped iterators to their logical namespace.
- `00d16a040b` reduced coverage completeness across admissions with logical
  AND.
- `a83ed20b2b` removed the discovery persistence/read round trip and shared
  prepared descriptors.
- `8481b32152` replaced transactional fresh-build admission with complete
  bitmap accumulation and atomic `write_opt` publication.
- `a28892d022` completed forward-sort validation, bounded dispatch accounting,
  engine/process instrumentation and target-sensitive scheduling tests.
- `f9e0e7c94c` added Unicode durable reopen and retained bounded construction
  fixtures.

## Requirement disposition

| Second-repair requirement | Final disposition |
| --- | --- |
| Transaction and receipt identity | Retained through one atomic cohort receipt and exact replay digest. |
| Plan once and reuse across retries | Planning/digest is computed once; publication retries reuse the encoded actions. |
| Per-file mutation trees removed | Superseded and removed from the default path by complete-image accumulation. |
| Batch-wide typed block aggregation | Implemented in the bulk accumulator; no encoded key per FTS membership. |
| Linear record/fact association | Implemented with contiguous per-record fact ranges. |
| Dynamic resource ownership | Implemented for dispatched preparation estimates and measured chunk capacities; target-sensitive test passes. |
| Actual namespace bounds | Implemented and tested for absent, one-sided, prefix and all-FF ranges. |
| Actual forward sort rows | Validator and reference export consume `SortEntries`; corruption test fails validation. |
| Coverage AND and empty collections | Implemented from receipt summaries and runtime schema. |
| Bounded finalization | Implemented with grouped ordered scans and no reverse-contribution decode. |
| Fresh transaction RMW retry path | Superseded by the controlling third specification; production mutations keep the transactional path. |
| Native gate result | Performance failure; production integration remains stopped. |

## Validation

- `cargo check --workspace --all-targets`: passed.
- `cargo test --workspace`: passed.
- `cargo test -p objekte-search-core --features rocksdb-prototype --lib`:
  553 passed, 15 ignored.
- Detailed-profiling all-target check: passed.
- `cargo test --workspace --features objekte-search-core/rocksdb-prototype`:
  passed.
- Full ASCII validation: 148,486 files/records, 40 receipts, zero errors.

The final evidence and exact ratios are in the bulk-construction report and
`docs/ai/generated/rocksdb-bulk-construction-repair/comparison.json`.
