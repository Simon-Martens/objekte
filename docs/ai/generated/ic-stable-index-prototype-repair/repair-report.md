# Stable-index prototype repair report

Task: `ic-stable-index-prototype-repair`  
Date: 16 November 2026  
Reviewed prototype: `530f4037f01bcd38f5027042e54676b543fdd230`  
Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Repair branch: `integration/ic-stable-index-prototype-repair`  
Repair worktree: `/home/simon/source/objekte-integration-ic-stable-index-prototype-repair`

## Outcome

**Correct repaired prototype with residual performance and memory rejection.**
The repair materially improves the original prototype and closes the scheduling,
mutation, coverage, publication, descriptor and storage-codec findings. Two
architecture findings are only partially closed: the direct builder still
finalizes a native `CollectionState` graph before stable conversion, and some
owner families beyond postings still merge serially. They are recorded
explicitly in `findings-disposition.md`; this is not a near-parity claim.

Measured matched P=8 ASCII campaign:

| Measurement | Native | Original prototype | Repaired candidate |
| --- | ---: | ---: | ---: |
| Median source build | 1.649 s | 4.965 s | 3.225 s |
| Ratio vs native | 1.00x | 3.01x | **1.96x** |
| Improvement vs original | — | — | **35.0%** |
| Resident RSS | 394.8 MB | — | 760.9 MB (1.93x native) |
| Peak RSS | 408.0 MB | — | 828.0 MB (2.03x native) |
| Stable payload | — | — | 407.4 MB |

Unicode P=8, three paired samples: native 1.830 s, original 5.155 s, repaired
3.568 s; **1.95x native**, **30.8% better than original**. Resident/peak RSS
were 1.87x/1.93x native.

Scheduling-only isolated comparison (P=8, three paired fresh processes):
original median 5.005 s, scheduling candidate median 3.858 s; **22.9%**
improvement. Component makespan fell from 2.956 s in the reviewed trace to
1.911 s in the scheduling-only trace. Final trace makespan was 1.421 s.

Synthetic workload (100,000 records, 100,000 distinct keys, 1,000,000 tag
memberships, 40 components):

| Workers | Native | Repaired | Ratio |
| ---: | ---: | ---: | ---: |
| 1 | 1.590 s | 6.213 s | 3.91x |
| 2 | 1.010 s | 3.672 s | 3.64x |
| 8 | 0.764 s | 1.638 s | 2.15x |

## Main improvements

- Largest-first, one-component-per-Rayon-task scheduling with deterministic
  IDs and full trace metrics. Large file/path/record/identity catalogs now run
  on different workers from the first wave.
- Role-specific stored key types and separate tree variants: fixed-width
  u32/u8/u64 keys for file, record, state, gram and occurrence identities;
  unbounded typed wrappers only where values are genuinely variable. Key codec
  revision advanced to 3.
- Direct path avoids native ascending/descending order construction through a
  keys-only sort finalizer; configured composite sorts now have their own
  ordered components and native constituent comparison.
- `BatchContribution::into_parts` consuming decomposition; metadata admission is
  separate from index owners; posting-field fragments merge in parallel through
  the shared worker pool.
- Prepared file-change API with typed/text/state/sort/composite/relation facts,
  grouped identity/domain updates, cached FileId high water, typed identity
  policies, full path entries and zero-record path handling.
- Explicit `Restriction::{NoRestriction, Complete}` coverage contract, shared
  live-domain cache without bitmap cloning, and unusable-state enforcement.
- Atomic standalone publication through sibling temporary files; single
  streaming payload hash; staged generation metadata committed only after
  rename; component restoration failures enter `Failed` state.
- Checked component-ID decoding, dependency/identity/collection manifest
  validation, canonical configuration hashing without Debug formatting,
  text/alphabet/composite descriptors, component revisions.
- Observed build counters for inserts and bitmap encodes; memory length and Vec
  capacity recorded separately.

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
- core feature tests passed (45 focused prototype tests);
- workspace feature tests passed;
- full workspace tests passed except
  `backend::open::tests::incremental_catalogue_preserves_non_utf8_file_paths`,
  which also fails alone on the untouched native baseline in this environment;
  the full desktop library suite passes when run normally.

Full Musenalm direct-build validation passed with zero semantic differences:
148,486 files/records, 152,544 posting keys, 2,814,411 posting members,
25,477 gram keys, 56,275,908 gram memberships, 566,768 sort entries.

## Persistence and mutation

- Snapshot save: 232 ms for 1,323 components / ~407 MB.
- Manifest open: 30.5 ms; first 27.9 MB component load: 26.3 ms.
- Standalone open/save and copied/renamed manifest-free files remain supported.
- Primitive postings: 1.66-2.20 µs median, 2.31-3.70 µs p95 over 200 repetitions
  after component and live-domain preload.
- 65,536-member standalone mutation: 23.7 ms; save 23.3 ms.
- Ten replacements of a 200-record file: 2,000 dead slots; reopened payload grew
  from 1.18 MB to 1.57 MB, snapshot 1.77 MB. Physical reclamation remains
  deferred and is not called compaction.

## Residual limitations

1. **R02 partial.** Batch contributions are consumed and decomposed, but the
   direct path still finalizes native `CollectionState` indexes and borrows the
   collection graph during stable construction. That overlap explains the
   resident/peak RSS rejection. A fully owned conversion job graph remains the
   next required design step.
2. **R03 partial.** Posting field owners merge in parallel; custom bindings,
   packed bigram owners, sort-key owners and relation owners still merge in the
   coordinator.
3. **R19 partial.** Candidate primitives now use preload and median/p95
   distributions, but matched native primitive distributions were not run.
4. Configured composite sorts are implemented and tested on a synthetic
   fixture; the Musenalm profile itself has no composite registrations.
5. macOS/Windows execution was unavailable; only Linux was tested.
6. Logical deletion retains dead posting memberships; no physical compaction.

## Evidence layout

- Raw runs: `bench/results/ic-stable-index-prototype-repair/`
- Scheduling-only trace: `scheduling-only/`
- Final ASCII campaign: `final-ascii-p8/`
- Final Unicode campaign: `final-unicode-p8/`
- Findings: `docs/ai/generated/ic-stable-index-prototype-repair/findings-disposition.md`
- Campaign identity: `campaign-manifest.json`
- Comparison: `comparison.json`
