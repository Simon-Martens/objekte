# RocksDB prototype source and reuse manifest

Date: 2026-11-15
Task: `rocksdb-index-prototype`
Integration branch: `integration/rocksdb-index-prototype`
Frozen master/base: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
Read-only donor: `abandoned/redb-primary-index` at
`09445853ccbce1cdc690a6f1c62bfe765a72e774`

## Repository state

- Original checkout: `/home/simon/source/objekte`, branch `master`, at the
  frozen base with user-owned untracked documents preserved.
- Integration worktree:
  `/home/simon/source/objekte-integration-rocksdb-index-prototype`.
- Detached baseline worktree:
  `/home/simon/source/objekte-baseline-rocksdb-index-prototype`.
- The abandoned donor branch is read only. It is not merged, modified, pushed
  or checked out in either worktree.

## Toolchain

| Tool | Version |
| --- | --- |
| rustc | 1.99.1 (`58cf8f9c3`, 2026-10-13) |
| cargo | 1.99.1 (`5f94df478`, 2026-08-27) |
| GCC / G++ | 16.2.1 |
| Clang / Clang++ | 22.1.8 |
| libclang | 22.1 |
| CMake | 4.3.0 |
| GNU Make | 4.4.1 |
| pkg-config | 2.5.1 |

No `ROCKSDB_LIB_DIR` override is configured. The prototype will build the
binding's bundled native library.

Resolved locked engine:

| Crate | Resolved version |
| --- | --- |
| `rocksdb` | `0.25.0` |
| `librocksdb-sys` | `0.19.0+11.8.1` |

The first native build exhausted the 12.7-GiB user quota on `/tmp`. All final
builds set `TMPDIR=<worktree>/target/cc-tmp`, which is on the home filesystem.
This changes only compiler temporary-file placement; it does not substitute or
override the RocksDB native library.

## Frozen native baseline

Command:

```sh
target/release/examples/bench_build_musenalm \
  --root /home/simon/source/objekte-baseline-rocksdb-index-prototype/examples/musenalm \
  --alphabet ascii \
  --build-workers auto \
  --schema-manifest-path .../baseline/ascii/schema-ascii.json
```

Executable SHA-256:
`f18c038090941f1a583711111bcd3510381b7e346fa5b7acda7b29122a8fe6c0`.

| Measurement | Value |
| --- | ---: |
| Records | 148,486 |
| Caller build | 1,413,380,574 ns |
| Discovery | 339,661,291 ns |
| Process peak RSS | 486,240,256 bytes |
| Index signature | `75c4992e860d469d562b3b5a5f0101a5d655f211cbd747f774907e4d094a5f3e` |

Raw output and schema manifest are under
`bench/results/rocksdb-index-prototype/baseline/ascii/`.

## Donor extraction record

The following donor files are approved inputs to adapt, not files to copy
unchanged. Every implemented extraction will update this table with destination,
change summary and validation.

| Donor path at `09445853cc` | SHA-256 | Initial reuse decision |
| --- | --- | --- |
| `crates/core/src/index_store/key_codec.rs` | `b253c5ffac2303197f5d3d7f9f7e48362f1e17209c40a3edd9069f47f6b4b805` | Port ordered codecs; prepend RocksDB namespace at physical-key boundary. |
| `crates/core/src/index_store/bitmap_block.rs` | `189da11b65baa4014550190c7b8aafc5272360ea9e61aba9fe205fa7bab6fefd` | Reuse sparse/dense 4,096-slot bitmap format and malformed-input checks. |
| `crates/core/src/index_store/contribution_codec.rs` | `340580681f3373c182fed840a203fe4273bc851272689f00f33937f490c588df` | Port LZ4 envelope policy; add reusable scratch and retained-capacity accounting. |
| `crates/core/src/index_store/store_build_prepare.rs` | `6e96b63ed26a6a4ff7ede4cd35cec8c41f2635f0998c1818ae6914415da4421c` | Reuse flat fact shapes only; replace incomplete retained-capacity estimate. |

Additional donor algorithms will be extracted selectively from `keys.rs`,
`value_codec.rs`, `contributions.rs`, `posting_blocks.rs`, `states.rs`, `fts.rs`,
`sorting.rs`, `relations.rs`, `write.rs`, `read.rs`, `validation.rs`, `build.rs`
and `posting.rs` where they implement source-independent semantics. Redb
transactions, table handles, backend ownership, page accounting and lifecycle
publishing are excluded.

### Extracted in Phase 1

- `crates/core/src/rocksdb_store/key_codec.rs` and
  `key_codec.test.rs`: donor ordered encoding copied without semantic changes;
  the module-local `StoreError` is the prototype error type.
- `crates/core/src/rocksdb_store/bitmap_block.rs` and
  `bitmap_block.test.rs`: donor sparse/dense 4,096-slot codec copied without
  format changes; the module-local `StoreError` is the prototype error type.
- `crates/core/src/rocksdb_store/value_codec.rs`: prototype-specific format
  header plus selected donor metadata/contribution types. Legacy redb revision
  and construction metadata are not reused.
- `crates/core/src/rocksdb_store/contribution_codec.rs`: donor LZ4 policy
  adapted to unconditional prototype-feature use, borrowed serialization and
  reusable scratch buffers.
- New `namespace.rs`, `mod.rs` and `mod.test.rs` are prototype-native and have
  no redb dependency.

Phase 1 validation: 29 `rocksdb_store` tests pass. A memory-environment host
directory remains empty, a replacement memory environment recovers nothing,
snapshots retain their versions, concurrent transactions preserve distinct bits
in the same value, namespaces do not leak, and file mode survives WAL flush,
memtable flush, close and reopen.

### Extracted and adapted in Phase 2

- Donor `crates/core/src/build.rs` A4 direct-preparation concepts were adapted
  behind `rocksdb-prototype`: flat values, lengths, structural masks, sort
  projections, relations, coverage, file fingerprints and sorted normal/skip
  grams. The production native scheduler was restored to its master weights
  after an exploratory donor weight multiplier caused a measurable native
  control regression.
- Donor `store_build_prepare.rs` types were moved into
  `rocksdb_store/prepare.rs`; retained-capacity accounting now includes owned
  record/fact vectors and gram buffers. Final coverage is not taken from these
  provisional local counts.
- Donor `keys.rs` and `value_codec.rs` semantics were extended with prototype
  file, identity, receipt, coverage-delta, relation and namespace layouts.
- Donor block-change and reverse-contribution algorithms informed
  `rocksdb_store/write.rs`, but redb table access and transaction ownership were
  replaced by one ordered RocksDB mutation plan per bounded physical-file
  admission transaction.
- Donor `posting.rs` and `bigram.rs` fact extraction was adapted as
  feature-gated helpers. Master posting/bigram behavior remains unchanged when
  the feature is off.
- New `build.rs`, `finalize.rs`, `validation.rs` and `reference.rs` modules are
  prototype-native. Finalization recomputes exact cardinalities from completed
  block facts, and validation checks every forward membership against its
  reverse contribution.

Phase 2 validation: 31 `rocksdb_store` tests pass. This includes overlapping
collection membership, multi-record files, empty/malformed input, native
exact-fact fixture comparison, finalization and full reverse-membership
invariants. The final measured-corpus validation has zero errors.

| Matched ASCII measurement | Value |
| --- | ---: |
| Pristine native, P=8 | 936,001,020 ns |
| Candidate native control, P=8 | 967,230,805 ns |
| RocksDB W=4 readiness | 76,344,067,646 ns |
| Pristine native resident / peak | 394,117,120 / 404,692,992 B |
| RocksDB W=4 resident / peak | 4,766,621,696 / 5,054,058,496 B |

Decision: reject production migration under the early-exit rule.

## Required semantic adaptations

- Replace redb transaction state with pessimistic RocksDB `TransactionDB`.
- Use one default column family and a one-byte logical namespace registry.
- Replace commit revisions with snapshots, a shared build gate, unique batch
  receipts and explicit Finalizing/Complete phases.
- Do not update global directories or gram statistics in every admission.
- Recompute final coverage/cardinality statistics from completed facts.
- Count both outer vector capacity and nested owned buffers in preparation
  budgets.
- Reject inference and all deliberately deferred production integration.

## Baseline and donor protections

- The baseline worktree remains detached and unmodified.
- Donor files are read through Git object access only.
- Generated RocksDB database directories and copied corpora are not committed.
- Raw benchmark output remains under `bench/results/rocksdb-index-prototype/`.
