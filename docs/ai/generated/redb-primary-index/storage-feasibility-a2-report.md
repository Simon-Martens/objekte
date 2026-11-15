# redb primary index — A2 complete-store feasibility report

- Task: `redb-primary-index`
- Amendments: `objekte_redb_structural_state_amendment_a1.md`,
  `objekte_redb_storage_amendment_a2.md`
- Integration branch: `integration/redb-primary-index`
- Baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
- Date: 2026-11-15
- Status: **all four A2 variants were built and measured; the combined candidate is closest but the unchanged RSS gate still fails**

This report is the A2 repetition of Phase 5. The historical A1 campaign remains
in `state-block-feasibility-report.md` and is not relabelled. A1 used a
same-process native-drop measurement and omitted reverse gram rows; this
campaign uses a separate prepared input and a fresh consumer process.

## 1. What A2 added

- A two-process fixture path: `--prepare-input` may use the native repository;
  `--input` imports the complete stream into an empty in-memory store in a
  fresh process that never constructs a native repository.
- Complete per-record reverse gram lists, transposed from the native
  gram→bitmap index in 4,096-slot ranges and stored once in `record_grams`.
- `ContributionPayload` without gram vectors; `record_contributions` no longer
  serializes grams.
- Format tag 3 contribution envelope with independent per-record raw/LZ4
  selection under the `store-reverse-lz4` feature.
- Bounded `value_blocks`/`length_blocks` with the shared bitmap codec, plus
  grouped `PostingBatch` insertion.
- `CountingBackend` wrapper for backend length/peak measurement.
- Control variant retained through the `store-posting-rows` feature.

The four variants share one input stream and one binary source:

| Variant | Features | Reverse payload | Value/length |
| --- | --- | --- | --- |
| `a2-control` | `store-feasibility,store-posting-rows` | raw tag 3 | individual rows |
| `a2-reverse` | `store-feasibility,store-posting-rows,store-reverse-lz4` | raw/LZ4 | individual rows |
| `a2-postings` | `store-feasibility` | raw tag 3 | bounded blocks |
| `a2-combined` | `store-feasibility,store-reverse-lz4` | raw/LZ4 | bounded blocks |

## 2. Input identity and completeness

- Corpus: `/home/simon/source/objekte/examples/musenalm`.
- Records: 148,486; files: 148,486; collections: 7.
- Logical facts: 2,814,411 value memberships, 921,173 length memberships,
  14,004,908 state facts, 56,275,908 normal/skip gram memberships,
  566,768 sort entries (ASCII). Unicode has 58,058,087 gram memberships.
- The input manifest records corpus digest, configuration digest, source root
  and per-collection counts. Consumer import is a fresh process with no native
  repository.
- Reverse gram rows are populated for every record/binding/kind message; the
  sum of stored reverse lists equals the forward gram cardinality.

## 3. ASCII measurements (seven fresh processes per variant)

| Variant | Store-only RSS (median) | Peak RSS (median) | Table page bytes (median) | Import time (median) |
| --- | ---: | ---: | ---: | ---: |
| control | 1,174.3 MiB | 1,174.5 MiB | 806.2 MiB | 16.83 s |
| reverse | 1,174.5 MiB | 1,174.5 MiB | 705.4 MiB | 16.49 s |
| postings | 1,174.5 MiB | 1,174.7 MiB | 593.5 MiB | 8.66 s |
| **combined** | **630.4 MiB** | **630.7 MiB** | **492.5 MiB** | **8.76 s** |

B0 native baseline: 478.2 MiB post-build, 488.2 MiB peak. The unchanged gate
is `max(1.15× baseline, baseline + 32 MiB)` ≈ **550 MiB**.

Combined Unicode: 631.4 MiB RSS, 631.7 MiB peak, 525.3 MiB table pages,
8.98 s import.

The combined candidate is the best variant by both RSS and pages, but it is
still about 80 MiB above the gate. Table pages plus the configured 64 MiB cache
are 556.5 MiB before backend metadata, so the page budget alone is marginally
over the limit.

## 4. Combined candidate table breakdown (ASCII)

| Table | Stored bytes | Page bytes |
| --- | ---: | ---: |
| `record_grams` | 70.00 MiB | 108.02 MiB |
| `record_contributions` | 88.82 MiB | 101.25 MiB |
| `sort_entries` | 46.74 MiB | 88.12 MiB |
| `fts_blocks` | 42.11 MiB | 74.27 MiB |
| `files` | 22.35 MiB | 24.52 MiB |
| `value_blocks` | 12.26 MiB | 20.64 MiB |
| `value_directory` | 11.89 MiB | 19.30 MiB |
| `records` | 12.60 MiB | 14.06 MiB |
| `domain_members` | 2.51 MiB | 7.21 MiB |
| `state_blocks` | 2.24 MiB | 4.03 MiB |
| all tables | 335.0 MiB | 492.5 MiB |

Backend logical length and peak from the counting wrapper: **570.4 MiB**.
The 78 MiB difference between backend length and table page bytes is redb
metadata/free pages; process RSS adds the configured cache and allocator
overhead.

### Savings by change

- Value rows → blocks: 163.2 → 20.6 MiB pages.
- Length rows → blocks: 70.5 → 0.5 MiB pages.
- Raw → LZ4 reverse contributions: 202.2 → 101.3 MiB pages.
- State rows → blocks (A1): 14.0 M rows → 4.0 MiB pages.
- Reverse grams are now present (108 MiB pages) rather than omitted.

## 5. Correctness

- `cargo test -p objekte-search-core --lib index_store`: 85 tests pass.
- Control-feature tests (`store-posting-rows,store-reverse-lz4`): 84 pass.
- Tests cover block boundaries, cardinalities, directory counts, state masks,
  contribution raw/LZ4 envelopes, truncation/malformed lengths, value/length
  prefix isolation, abort, same-slot replacement, and reverse-mask round trips.
- Full-store validation on the imported combined candidate completed with
  **zero errors**: 7 collections, 148,486 live records, 2,814,411 value
  memberships, 921,173 length memberships, 14,004,908 state facts, 169,856 FTS
  blocks and 566,768 sort entries. The validation process also checked that
  reverse gram memberships equal forward gram cardinalities and that state
  blocks agree with reverse masks and statistics.
- No production query path is routed through the store; end-to-end query gates
  remain unavailable, not passed.

## 6. Gate evaluation

| Gate | Result |
| --- | --- |
| Complete input and one-copy reverse grams | pass |
| Control/postings/reverse/combined variants built and measured | pass |
| Block/contribution codec correctness | pass |
| Store-only post-build RSS ≤ ~550 MiB | **fail** (630.4 MiB best) |
| Peak RSS ≤ ~590 MiB | **fail** (630.7 MiB best) |
| Backend length ≤ gate | **fail** (570.4 MiB) |
| Phase-5 primitive feasibility | pass in earlier A1 measurements; no regression observed |
| End-to-end query/CRUD gates | unavailable (no production cutover) |
| Production cutover | blocked |

No variant passes. A2.8 requires remaining in Phase 5 and reporting the next
dominant allocations rather than broadening the representation or relaxing the
gate implicitly.

## 7. Next dominant families and required decision

Combined page bytes: `record_grams` 108.0, `record_contributions` 101.3,
`sort_entries` 88.1, `fts_blocks` 74.3, identity/catalog and value blocks
remainder. To reach the gate, a further explicit amendment would be needed for
at least one of:

1. **Compress `record_grams`** (per-record delta/varint list or an LZ4-framed
   list) or fold it into a single compressed reverse payload.
2. **Compress or restructure `sort_entries`**, e.g. shared ICU key dictionaries.
3. **Compress `fts_blocks`** or change the block format (currently mandated to
   remain unchanged).
4. **Relax the RSS gate** to accept approximately 1.32× the best candidate
   (630 MiB vs 478 MiB baseline) or a matched higher number.

The recommended next step is an explicit amendment for reverse gram and/or sort
compression, followed by another Phase-5 cycle. Phases 6–12 remain blocked.

## 8. Evidence

- Raw campaigns: `bench/results/redb-primary-index/feasibility-a2/<variant>/<profile>/`.
- Full validation: `bench/results/redb-primary-index/feasibility-a2/combined/ascii/validation.json`.
- Input fixture: `/tmp/opencode/a2/ascii.input`, `/tmp/opencode/a2/unicode.input`
  (benchmark fixtures, not production storage; recreate with the recorded
  `--prepare-input` command).
- Machine-readable gate: `docs/ai/generated/redb-primary-index/comparison-storage-a2.json`.
- Commands:

```sh
cargo build --release --locked -p objekte-search-core \
  --example bench_index_store --features store-feasibility

# producer (native reference process)
target/release/examples/bench_index_store --prepare-input /tmp/a2/ascii.input \
  --root /home/simon/source/objekte/examples/musenalm --alphabet ascii

# combined consumer (fresh process)
cargo build --release --locked -p objekte-search-core \
  --example bench_index_store --features store-feasibility,store-reverse-lz4
target/release/examples/bench_index_store --input /tmp/a2/ascii.input
```

## 9. A3 decision and revised gate

`objekte_redb_memory_budget_amendment_a3.md` adopts the combined
representation and replaces the resident/peak factors with 1.40/1.60 while
keeping correctness, speed, CRUD and stability gates.

| Gate | Old limit | A3 limit | Combined |
| --- | ---: | ---: | ---: |
| Post-build RSS | 549,953,331 B (524.5 MiB) | 669,508,403 B (638.5 MiB) | 661,000,192 B (630.4 MiB) |
| Peak RSS | 585,891,840 B (558.7 MiB) | 781,189,120 B (745.0 MiB) | 661,348,352 B (630.7 MiB) |

Under A3 the combined candidate passes both memory limits. The original 1.15
and 1.20 results remain failures and are not relabelled. Backend length
(570.4 MiB) and page-plus-cache totals are retained as diagnostics, not
standalone RSS gates.

The A2-specific primitive and mutation evidence required by A3 is recorded in
`bench/results/redb-primary-index/feasibility-a3/ascii/primitives-mutations.json`:
point membership 0.34 us, value lookup 1.40 us, state scan 2.58 us, FTS
candidate 2.11 us, restricted candidates 5.90 us, sort page 20.3 us, single
record update p50/p95/p99 112/118/124 us, and delete/reinsert p50/p95/p99
2.88/3.58/3.99 ms. The relative native baseline comparison is unavailable
because no equivalent native primitive harness exists; the Phase-5 early
feasibility bound is therefore recorded as absolute evidence rather than a
claimed ratio. Phase 5 is closed; Phase 6 starts from the A2 combined
implementation commit on `integration/redb-primary-index`. Production
construction, query, CRUD and frontend paths remain unimplemented and must
still pass their unchanged gates.

## 10. A3 decision record

- Selected representation: `a2-combined` (A1 state blocks, A2 value/length
  blocks, per-record raw/LZ4 contributions, one-copy reverse grams).
- Revised resident factor: 1.40; revised peak factor: 1.60. Absolute
  allowances remain 32/64 MiB.
- ASCII limits: post-build 669,508,403 B, peak 781,189,120 B. Combined:
  661,000,192 B / 661,348,352 B. Both pass.
- Historical 1.15/1.20 failures remain recorded and are not relabelled.
- Phase 5 is closed. Phase 6 starts from the A2 combined implementation on
  `integration/redb-primary-index`.
