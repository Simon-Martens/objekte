# redb primary index — phase 5 feasibility report

- Task: `redb-primary-index`
- Integration branch: `integration/redb-primary-index`
- Worktree: `/home/simon/source/objekte-integration-redb-primary-index`
- Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9` (`master`)
- Date: 2026-11-15
- Status: **feasibility gate failed on memory; production cutover not started**

This report covers Phases 1–5 of `docs/ai/tasks/objekte_redb_primary_index_implementation_plan.md`.
The store foundation and a temporary native-to-redb export bridge are
implemented and tested. The bridge is a measurement tool, not a production
path, and is compiled only with the `store-feasibility` feature.

## 1. What is implemented

`crates/core/src/index_store/` contains one in-memory redb database per index
instance with the complete registered table schema:

| Area | Module | State |
| --- | --- | --- |
| Database, ownership, snapshot view | `mod.rs`, `read.rs` | implemented |
| Table registry and metadata | `tables.rs` | implemented |
| Ordered scalar/numeric/path codecs | `key_codec.rs`, `keys.rs` | implemented and property-tested |
| Versioned metadata and contribution encoding | `value_codec.rs` | implemented |
| Slot free list and incarnations | `slot_allocator.rs` | implemented |
| Contribution set differences | `contributions.rs` | implemented |
| Structured memberships and directories | `write.rs`, `read.rs` | implemented |
| FTS 4,096-slot sparse/dense blocks and intersection | `fts.rs` | implemented |
| Configured sort projections | `sorting.rs` | implemented |
| Relation outgoing/incoming rows and states | `relations.rs` | implemented |
| Discovery/admission state and counters | `construction.rs` | implemented |
| Atomic file admission, update, delete, move | `mutations.rs` | implemented |
| Complete table deletion/recreation | `regeneration.rs` | implemented |
| Offline invariant validation | `validation.rs` | implemented |
| Native export bridge (temporary) | `feasibility.rs` | feature-gated, measurement only |

`cargo test -p objekte-search-core --lib index_store` passes 58 tests,
including 100,000 generated numeric comparison pairs, numeric extremes,
malformed codec inputs, FTS cardinality boundaries 1/255/256/4096, block
statistics, slot reuse/incarnation rejection, snapshot retention across reset,
abort atomicity, atomic file admission/update/delete, relation target reuse,
and full store invariants.

Commit: `Introduce in-memory redb index store and ordered codecs`.

## 2. Environment

- `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1 (5f94df478 2026-08-27)`
- 32 logical workers, Linux, NVMe storage.
- Corpus: `/home/simon/source/objekte/examples/musenalm` (148,486 original
  JSON files, 7 collections, historical Musenalm schema).
- Release profile, `redb = 4.6.0`, `InMemoryBackend`, 64 MiB page cache.
- Baseline B0 binaries and workspace tests were built at the pinned commit;
  `cargo test --workspace --locked --no-fail-fast` exited 0.

### Commands

```sh
# Native baseline construction
target/release/examples/bench_build_musenalm \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii --build-workers auto \
  --schema-manifest-path bench/results/redb-primary-index/b0/schema-ascii.json

# Feasibility export (ASCII historical, Unicode historical, ASCII extended relations)
cargo build --release --locked -p objekte-search-core \
  --example bench_index_store --features store-feasibility
target/release/examples/bench_index_store \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii \
  --result-dir bench/results/redb-primary-index/feasibility
target/release/examples/bench_index_store --root ... --alphabet unicode ...
target/release/examples/bench_index_store --root ... --alphabet ascii --extended ...
```

Raw JSON evidence is in `bench/results/redb-primary-index/feasibility/`.
Baseline build evidence is in
`/home/simon/source/objekte-task-redb-primary-index-baseline/bench/results/redb-primary-index/b0/`.

## 3. Native baseline

| Run | Wall time | Post-build RSS | Peak RSS | Records |
| --- | ---: | ---: | ---: | ---: |
| B0 ASCII | 0.993 s | 478.2 MB | 488.2 MB | 148,486 |
| B0 Unicode | 1.106 s | 493.9 MB | 504.1 MB | 148,486 |

The corpus is entirely in the OS page cache on this host; historical
cold-cache numbers are not comparable. Both revisions are measured under the
same conditions.

## 4. Store contents after export

### ASCII historical schema

| Table | Rows |
| --- | ---: |
| files, file_build_state, paths, query_paths | 148,486 each |
| records, logical_ids, file_records | 148,486 each |
| value_members | 2,814,411 |
| length_members | 921,173 |
| **state_members** | **14,004,908** |
| value_directory | 152,544 |
| length_directory | 641 |
| domain_members | 292,691 |
| sort_definitions | 16 |
| sort_entries | 566,768 |
| fts_blocks | 169,856 |
| fts_gram_statistics | 25,477 |
| relation_outgoing / relation_incoming | 0 |
| relation_states | 0 |
| fields / bindings | 418 / 433 |

FTS logical membership bits: 56,275,908. Distinct gram keys: 25,477.

### ASCII with extended relation schema

Adds 228,503 relation occurrences in each direction and 346,799 relation
state rows; value members rise to 2,908,927 because the extended schema
registers additional typed fields.

### Unicode historical schema

Same row counts as ASCII; FTS membership bits rise to 58,058,087.

## 5. Memory

| Configuration | RSS after export | RSS after dropping native index | Peak |
| --- | ---: | ---: | ---: |
| ASCII historical | 1,679.8 MB | 1,625.1 MB | 1,679.8 MB |
| Unicode historical | 1,680.6 MB | 1,612.8 MB | 1,680.6 MB |
| ASCII extended | 1,718.9 MB | 1,647.8 MB | 1,718.9 MB |

B0 post-build RSS is 478–494 MB. The store-only resident set is therefore
**3.3–3.4× the native baseline** and roughly 1.1 GB above it.

The 64 MiB redb page cache is not the cause: 14.0 M individual
`state_members` rows dominate the database. At roughly 70–80 bytes per redb
row including tree overhead, the state family alone accounts for about
1.0–1.1 GB. Values, lengths, identity and catalog rows account for the rest.
The estimated reverse `record_grams` payload (112.6 MB for ASCII) and
`record_contributions` are additional costs not included in the exported
store.

## 6. Read and mutation primitives

Measured against the exported store (ASCII extended run, warm process):

| Operation | Observed |
| --- | ---: |
| 1,326-member value lookup (200 repetitions) | 17.9 ms → **89 µs/query** |
| 4,010-slot state scan (200 repetitions) | 43.2 ms → **216 µs/scan** |
| Sort page, 100 entries (50 repetitions) | 1.04 ms → **21 µs/page** |
| FTS gram candidates, 940 results (20 repetitions) | 36 µs → **1.8 µs/query** |
| Record point lookup (100 repetitions) | 37 µs → **0.37 µs/lookup** |
| Synthetic 1,000-record update, 2 rounds | 25.8 ms → **12.9 ms/round** |
| Synthetic 1,000-record delete, 2 rounds | 38.5 ms → **19.2 ms/round** |

These are storage-primitive costs, not end-to-end query latencies. No query
path has been routed through the store yet, so the 1.10× query aggregate gate
and the individual per-case gates are **unavailable**, not passed.

## 7. Gate evaluation

| Gate | Result | Evidence |
| --- | --- | --- |
| Store correctness (foundation tests) | **pass** | 58 tests |
| Store invariant validation | **pass** | `validation::validate` tests |
| Table schema/row coverage of historical corpus | **partial** | bridge omits contributions, record grams, coverage |
| Post-build RSS ≤ max(1.15× B0, B0 + 32 MiB) | **fail** | 1.61–1.65 GB vs 478–494 MB |
| Peak construction RSS ≤ max(1.20× B0, B0 + 64 MiB) | **fail** | 1.68–1.72 GB vs 488–504 MB |
| Query aggregate / per-case latency | **unavailable** | no production query routing |
| CRUD latency vs native | **unavailable** | native comparison not captured; store primitives only |
| Progressive query correctness | **unavailable** | not implemented |
| Regeneration time | **unavailable** | store-level reset only |
| Legacy removal | **not started** | production still native |

## 8. Conclusion and required decision

The mandated representation stores every structured membership and every
structural state as an individual redb row. On the canonical corpus that is
17.7 M structured/state rows (14.0 M of them state facts), and the resulting
resident cost cannot meet the specified memory gate. This is a data-model
cost, not an implementation bug: changing key order, cache size or batch
scheduling within the current specification does not remove the rows.

Phase 5 explicitly forbids starting the broad production cutover before this
result is reviewed, and requires an explicit amended decision for a different
structured payload or a relaxed target. The work therefore stops before
Phases 6–12.

To resume, the user must choose one of:

1. **Amend the structured/state representation** (recommended): allow
   compressed per-field state blocks or bitmaps in addition to individual
   value memberships, then rerun this experiment. This is the only option
   with a plausible path to the memory gate.
2. **Accept a larger memory budget**: keep individual state memberships and
   revise the post-build/peak RSS gates to roughly 3.5× the native baseline.
3. **Stop the migration**: keep the store foundation and bridge for future
   work without a production cutover.

No production consumer was changed. `Index`, `Repository`, query execution,
lifecycle mutations and frontends still use the native index.

## 9. Known bridge limitations

- `record_contributions`, `record_grams`, `field_coverage` and
  `coverage_members` are not populated by the bridge. Their cost is estimated
  or omitted, so the measured store is a lower bound for the full design.
- The bridge reads the final native gram→bitmap maps, so it cannot reconstruct
  per-record gram lists cheaply. The 112.6 MB estimate assumes about two bytes
  per membership in `record_grams`.
- Composite sort orders are not exported.
- No production query path is routed through the store; primitive timings are
  not end-to-end query results.
- The baseline query campaigns (`verify_jq_oracle`, 150/220-case suites) were
  not rerun because the memory gate already blocks cutover and the candidate
  has no equivalent query path yet. They remain mandatory before any future
  acceptance run.
