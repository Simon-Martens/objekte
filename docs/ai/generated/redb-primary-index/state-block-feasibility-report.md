# redb primary index — A1 structural-state block feasibility

- Task: `redb-primary-index`
- Amendment: `objekte_redb_structural_state_amendment_a1.md`
- Integration branch: `integration/redb-primary-index`
- Worktree: `/home/simon/source/objekte-integration-redb-primary-index`
- Baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
- Date: 2026-11-15
- Status: **A1 removed the diagnosed state-row cost; the unchanged RSS gate still fails**

This report is the A1 repetition of Phase 5. It preserves the original
individual-state failure in `feasibility-report.md` and adds a separate
candidate. No production consumer was changed.

## 1. What A1 changed

- Added `index_store/bitmap_block.rs`, the single canonical 4,096-slot
  sparse/dense bitmap codec, and moved FTS to it. FTS bytes and tests are
  unchanged.
- Removed the physical `state_members` table.
- Added `state_blocks` keyed `(collection, binding, predicate, block)` and
  `state_statistics` with exact cardinality and nonempty-block count.
- Replaced `StoredContributions.states` with sorted unique
  `(binding, u16 state mask)` pairs. Contribution records use a new format
  tag; obsolete individual-state encodings are rejected.
- Added grouped block mutation, exact statistics, semantic block iteration,
  point membership, cardinality reads and validation.
- Updated the temporary export bridge to write state blocks, compact masks,
  value/length/sort/relation contributions and state statistics.

Commit boundary: `Replace structural state rows with bounded redb bitmap blocks`.

## 2. Correctness

- `cargo test -p objekte-search-core --lib index_store`: **69 passed, 0 failed**.
- Full workspace tests: **passed**.
- Workspace all-target check: **passed**.
- State tests cover offsets 0/4095, blocks 4095/4096/4097, sparse 255 →
  dense 256 → sparse 255 transitions, multiple predicate bits, statistics,
  no-change updates, delete/reuse in one transaction, abort, reverse-mask
  round trips and invalid mask rejection.
- FTS codec/candidate tests still pass through the shared codec.
- Exact state facts on Musenalm match the failed candidate: **14,004,908**.

## 3. Storage shape after A1 (ASCII historical)

| Table | Rows | Stored bytes | Page bytes |
| --- | ---: | ---: | ---: |
| **state_blocks** | **4,877** | **2.34 MiB** | **2.65 MiB** |
| state_statistics | 1,243 | 0.03 MiB | 0.05 MiB |
| record_contributions | 148,486 | 160.9 MiB | 202.0 MiB |
| value_members | 2,814,411 | 65.0 MiB | 89.4 MiB |
| sort_entries | 566,768 | 46.7 MiB | 88.0 MiB |
| fts_blocks | 169,856 | 42.1 MiB | 74.9 MiB |
| length_members | 921,173 | 28.0 MiB | 36.5 MiB |
| files | 148,486 | 22.3 MiB | 24.5 MiB |
| records | 148,486 | 12.6 MiB | 14.1 MiB |
| value_directory | 152,544 | 11.9 MiB | 13.6 MiB |
| query_paths | 148,486 | 5.6 MiB | 7.0 MiB |
| paths | 148,486 | 5.4 MiB | 6.7 MiB |
| file_build_state | 148,486 | 4.8 MiB | 6.1 MiB |
| logical_ids | 148,486 | 4.7 MiB | 6.0 MiB |
| all tables (sum) | — | 417.4 MiB | 580.5 MiB |

`record_contributions` stores 167,497,999 encoded bytes of reverse
contributions (values, lengths, compact masks, sort projections and relation
occurrences). The bridge cannot reconstruct per-record FTS gram lists from the
native gram→bitmap representation; `record_grams` remains omitted and is
estimated at 112.6 MiB.

The diagnosed state cost is gone: 14.0 M facts now occupy 4,877 state blocks
and 2.65 MiB of pages, approximately a 300× page reduction for that family.
The bridge's export time fell from ~22.7 s to ~12.7 s.

## 4. Memory and the unchanged gate

Two measurement modes are reported because the export bridge retains the
native repository and temporary per-collection accumulators:

| Mode | Post-export RSS | Store-only RSS after dropping native | Peak RSS |
| --- | ---: | ---: | ---: |
| Default allocator, ASCII | 2,217.2 MB | **2,149.5 MB** | 2,217.2 MB |
| `MALLOC_ARENA_MAX=1` + trim, ASCII | 2,028.4 MB | **1,232.7 MB** | 2,028.4 MB |
| `MALLOC_ARENA_MAX=1` + trim, Unicode | 2,032.4 MB | **1,232.7 MB** | 2,032.4 MB |
| `MALLOC_ARENA_MAX=1` + trim, ASCII extended | 2,047.7 MB | **1,232.0 MB** | 2,100.2 MB |

Baseline B0 is 478.2 MB post-build (ASCII) and 493.9 MB (Unicode); the
unchanged gate is `max(1.15× baseline, baseline + 32 MiB)` ≈ **550 MB**.

Even using the smaller trimmed store-only sample, the candidate is
**1.23 GB**, about 2.6× the baseline. Table page bytes alone are 580.5 MiB;
adding the configured 64 MiB redb page cache gives 644.5 MiB before backend
capacity, allocator retention, transactional metadata and retained MVCC state.
The gate therefore fails before any process-overhead accounting.

## 5. Primitive performance

| Operation | Failed individual-row candidate | A1 |
| --- | ---: | ---: |
| State scan, 4,010 rows (200 repetitions) | 216 µs/scan | **3.2 µs/scan** |
| 1,326-member value lookup | 89 µs/query | 87 µs/query |
| Sort page, 100 entries | 21 µs/page | 22 µs/page |
| FTS candidates, 940 results | 1.8 µs/query | 1.7 µs/query |
| Synthetic 1,000-record update | 12.9 ms/round | 13.4 ms/round |
| Synthetic 1,000-record delete | 19.2 ms/round | 22.1 ms/round |

The state representation change delivers the expected read win without a
measurable regression in the other primitives.

## 6. Next dominant costs

1. `record_contributions`: 202.0 MiB pages. Mandated exact reverse data for
   source-free deletion; it duplicates value/length keys by design.
2. `value_members`: 89.4 MiB pages for 2.81 M individual value memberships.
3. `sort_entries`: 88.0 MiB pages for both directions of every configured
   order.
4. `fts_blocks`: 74.9 MiB pages; format is unchanged by A1.
5. `length_members`: 36.5 MiB pages.

These families together account for 490.8 MiB of the 580.5 MiB page total.
The unchanged gate cannot pass without amending at least one of them or
relaxing the RSS target. A1 explicitly forbids broadening the representation
silently.

## 7. A1 gate result

| Gate | Result |
| --- | --- |
| State blocks/codec correctness and FTS byte compatibility | pass |
| Exact state-fact parity (14,004,908) | pass |
| Removal of physical `state_members` production path | pass |
| Source-free deletion through compact masks | pass in store tests |
| Fresh store-only post-build RSS ≤ ~550 MB | **fail** (1.23 GB best case) |
| Peak RSS ≤ ~590 MB | **fail** (2.03–2.22 GB) |
| Phase-5 primitive feasibility (≤2× baseline + 100 µs) | pass for measured primitives; no end-to-end query path |
| Production cutover readiness | blocked |

## 8. Decision required

The A1 experiment validated the diagnosis and the block representation, but
the full store still cannot meet the unchanged memory budget. To continue,
choose one:

1. **Amend reverse/value/length representation (recommended for a further
   Phase-5 cycle).** Permit compact per-field value/length blocks or a
   compressed reverse-contribution payload. This is the next largest saving
   and would require a new amendment, because it changes fixed decision 2 for
   families A1 deliberately left alone.
2. **Relax the RSS gate.** Accept approximately 2.6× baseline (1.23 GB) or a
   matched higher number, with an explicit decision.
3. **Stop the migration.** Keep the foundation, A1 state blocks and evidence
   without a production cutover.

Phases 6–12 remain blocked until one option is approved. The original failed
candidate, B0/B1 evidence, this A1 candidate and the blocked task document are
retained separately by commit and evidence path.

## 9. Evidence

- Raw A1 JSON: `bench/results/redb-primary-index/feasibility-a1/`.
- Original failed campaign: `bench/results/redb-primary-index/feasibility/` and
  `docs/ai/generated/redb-primary-index/feasibility-report.md`.
- Machine-readable A1 gate: `docs/ai/generated/redb-primary-index/comparison-state-blocks.json`.
- Commands:

```sh
cargo build --release --locked -p objekte-search-core \
  --example bench_index_store --features store-feasibility
MALLOC_ARENA_MAX=1 target/release/examples/bench_index_store \
  --root /home/simon/source/objekte/examples/musenalm --alphabet ascii \
  --result-dir bench/results/redb-primary-index/feasibility-a1
```

The default-allocator run was executed with the same command without
`MALLOC_ARENA_MAX=1`.
