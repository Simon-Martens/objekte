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
