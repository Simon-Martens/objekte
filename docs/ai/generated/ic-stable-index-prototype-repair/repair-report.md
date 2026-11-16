# Stable-index prototype repair report

Task: `ic-stable-index-prototype-repair`  
Date: 16 November 2026  
Reviewed prototype: `530f4037f01bcd38f5027042e54676b543fdd230`  
Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Final code commit: `956648ae3075a85fbf76d3e4ecab8e349103df84`  
Repair branch: `integration/ic-stable-index-prototype-repair`  
Repair worktree: `/home/simon/source/objekte-integration-ic-stable-index-prototype-repair`

## Outcome

**Correct repaired prototype with residual performance and memory rejection.**
All R01-R20 findings now have a fixed disposition. The final repair removes the
borrowed native `CollectionState` conversion graph, moves completed native owners
into independent component jobs, merges every index family outside the metadata
coordinator in bounded waves, reports unavailable diagnostics honestly, and adds
matched native/stable primitive distributions.

Measured matched fresh-process P=8 ASCII campaign:

| Measurement | Native | Original prototype | Repaired candidate |
| --- | ---: | ---: | ---: |
| Median source build | 1.573 s | 4.717 s | 3.174 s |
| Ratio vs native | 1.00x | 3.00x | **2.02x** |
| Improvement vs original | — | — | **32.7%** |
| Resident RSS | 394.3 MB | — | 750.1 MB (1.90x native) |
| Peak RSS | 407.4 MB | — | 779.6 MB (1.91x native) |
| Stable payload | — | — | 408.6 MB |

Unicode P=8, three paired samples: native 1.878 s, original 5.323 s, repaired
3.661 s; **1.95x native**, **31.2% better than original**. Resident/peak RSS
were 1.81x/1.83x native.

Scheduling-only isolated comparison (P=8, three paired fresh processes):
original median 5.005 s, scheduling candidate median 3.858 s; **22.9%**
improvement. Final component traces report 1.704 s ASCII / 1.635 s Unicode
makespan, 4.91 / 5.04 average concurrent jobs, and large catalogs overlapping on
different workers.

Synthetic workload (100,000 records, 100,000 distinct keys, 1,000,000 tag
memberships, 40 components):

| Workers | Native | Repaired | Ratio |
| ---: | ---: | ---: | ---: |
| 1 | 1.702 s | 7.426 s | 4.36x |
| 2 | 1.065 s | 4.197 s | 3.94x |
| 8 | 0.787 s | 1.685 s | 2.14x |

## Final repair changes

- **Owned conversion (R02).** `source_build.rs` no longer creates a
  `CollectionState` graph. It finalizes owned records, postings, custom postings,
  packed bigrams, sort keys and relations, resolves relations through target
  identity lookups, and hands `OwnedCollection` values to
  `stable_index/build.rs::from_owned`. Component jobs own their maps and drop
  them after encoding. The borrowed `from_collections` adapter remains only for
  tests/transfer convenience.
- **Parallel owner routing and bounded waves (R03).** Extraction is split into
  waves of at most 2P jobs and a 64 MiB estimated preparation target, with one
  oversized-job exception. Metadata admission remains deterministic. Posting,
  custom-posting, raw/fallback/custom bigram, sort-key and relation fragments
  are merged in parallel across independent owners after each wave. The final
  ASCII run used 37 waves, zero oversized jobs and 149 ms aggregate owner-merge
  time.
- **Truthful diagnostics (R18).** Actual batch source reads and bytes are now
  aggregated; the final run reports 148,486 reads and 193,297,230 bytes rather
  than zero. Detailed-only timings serialize as `null` with explicit unavailable
  reasons when `detailed-profiling` is disabled. Loaded components now report
  live memory length and Vec capacity through their shared memory statistics;
  unloaded components report `null`.
- **Matched primitives (R19).** New plan/native/stable primitive modes measure
  point, range, length point/range, state, FTS candidates, sort pages and
  relation state/target operations on the same extended-relation Musenalm
  snapshot. 503 rows x 200 repetitions were run with preloaded dependencies,
  consumed results and propagated errors. Result membership matched exactly.

## Primitive distributions

Stable/native median-of-medians, 200 repetitions each:

| Family | Rows | Native | Stable | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Point | 119 | 70 ns | 1.81 µs | 25.9x |
| Bounded range | 40 | 6.87 µs | 22.15 µs | 3.22x |
| Length point | 43 | 60 ns | 1.64 µs | 27.4x |
| Length range | 17 | 6.91 µs | 15.97 µs | 2.31x |
| State | 224 | 50 ns | 701 ns | 14.0x |
| FTS candidates | 14 | 721 ns | 1.45 µs | 2.02x |
| Sort page (64) | 16 | 30 ns | 15.22 µs | 507x |
| Relation state | 24 | 30 ns | 747 ns | 24.9x |
| Relation target | 6 | 121 ns | 1.66 µs | 13.8x |

Result-membership mismatches: **0 / 503**. The large sort gap includes decoding
stable sort-key payloads; native returns a prefix of an already resident order
vector. These measurements do not support a stable primitive-parity claim.

## Verification

Required checks run on the repair branch:

```text
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
cargo test -p objekte-search-core --features ic-stable-prototype
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/ic-stable-prototype
```

Results:

- workspace check passed;
- core feature/profiled checks passed;
- core feature tests passed (46 focused prototype tests);
- workspace feature tests passed;
- the workspace test run has one failure,
  `backend::open::tests::incremental_catalogue_preserves_non_utf8_file_paths`.
  The same test fails alone in the untouched native baseline worktree at
  `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`; all other workspace tests pass.

Full Musenalm direct-build validation passed with zero semantic differences:
148,486 files/records, 152,544 posting keys, 2,814,411 posting members,
25,477 gram keys, 56,275,908 gram memberships, 566,768 sort entries.

## Persistence and mutation

- Extended-relation snapshot save: 280 ms for 1,341 components.
- Manifest open in the primitive run: 29.8 ms.
- Standalone open/save and copied/renamed manifest-free files remain supported.
- 65,536-member standalone mutation: 23.7 ms; save 23.3 ms.
- Ten replacements of a 200-record file: 2,000 dead slots; reopened payload grew
  from 1.18 MB to 1.57 MB, snapshot 1.77 MB. Physical reclamation remains
  deferred and is not called compaction.

## Residual limitations

1. Resident RSS is 1.90x ASCII / 1.81x Unicode native and peak RSS is 1.91x /
   1.83x, above the 1.40x/1.60x targets. All completed native collections are
   still finalized before the stable job graph is published. A further reduction
   requires a streaming collection-to-component pipeline.
2. Stable primitive operations that decode individual payloads remain materially
   slower than resident native maps; configured sort pages are the largest gap.
3. Configured composite sorts are implemented and tested on a synthetic fixture;
   the Musenalm profile itself has no composite registrations.
4. macOS/Windows execution was unavailable; validation is Linux-only.
5. Logical deletion retains dead posting memberships; no physical compaction.

## Evidence layout

- Raw runs: `bench/results/ic-stable-index-prototype-repair/`
- Previous scheduling-only trace: `scheduling-only/`
- Previous final ASCII campaign: `final-ascii-p8/`
- Previous final Unicode campaign: `final-unicode-p8/`
- Final owned-owner ASCII campaign: `final-owned-r02-r03-ascii/`
- Final owned-owner Unicode campaign: `final-owned-r02-r03-unicode/`
- Final matched primitives: `final-primitives/`
- Findings: `docs/ai/generated/ic-stable-index-prototype-repair/findings-disposition.md`
- Campaign identity: `campaign-manifest.json`
- Comparison: `comparison.json`
