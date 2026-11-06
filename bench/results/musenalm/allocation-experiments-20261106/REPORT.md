# Musenalm index-build allocation experiments

Starting HEAD: `e7de81d092e8a063adf9751479f64a9f32271062`. The current core and build example match the recorded `d15b689d1ac21709195b79fc382c045d8dec9867` source. Reference: `../parallel-index-construction-d15b689d1a/auto/`, ASCII, snapshots disabled, 32 automatic build workers, three-process median. Its source-metadata and schema fingerprints match the current corpus (`eec50dbf` and `041a1bb2` prefixes). All builds use release mode, rustc 1.99.1, the same 32-CPU affinity and performance governor. The full fingerprints, command, environment, raw JSON, stderr and schema manifest are saved beside this report.

`wall_ns` is the primary repository-opening interval; the extraction/index phase includes more than parsing and is not summed worker CPU time. RSS measures resident memory, not allocation counts. Each experiment has one post-change sample, so small timing changes are inconclusive.

| Experiment | Code SHA | Comparison SHA | Build wall ms | Delta ms | Delta % | Extraction/index ms | Finalization ms | Peak RSS MiB | Signature | Decision |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| Reference | d15b689d1a | — | 1689.657 | — | — | 1250.276 | 119.429 | 303.016 | 52140243 | recorded median |
| 1. Flat file-record mapping | 12e2573 | d15b689d1a | 1655.289 | -34.369 | -2.0% | 1210.853 | 113.099 | 285.367 | 52140243 | retain provisionally |
| 2. Scalar temporary allocations | 21322c1 | 12e2573 | 1612.440 | -42.849 | -2.6% | 1165.276 | 114.425 | 287.484 | 52140243 | retain provisionally |
| 3. Borrowed logical text | 6b8d101 | 21322c1 | 1553.532 | -58.908 | -3.7% | 1110.439 | 119.898 | 283.539 | 52140243 | retain provisionally |
| 4. Reusable source buffer | d7eda17 | 6b8d101 | 1582.672 | +29.140 | +1.9% | 1139.756 | 113.019 | 288.688 | 52140243 | retain provisionally |

| Retained state | Cumulative wall delta from reference |
| --- | ---: |
| After experiment 1 | -34.369 ms (-2.0%) |
| After experiment 2 | -77.217 ms (-4.6%) |
| After experiment 3 | -136.125 ms (-8.1%) |
| After experiment 4 | -106.985 ms (-6.3%) |

| Experiment | Build ms | Discovery ms | Resident MiB | Workers | Records | Parses | Failed parses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Reference | 1689.572 | 306.620 | 292.566 | 32 | 148486 | 148486 | 0 |
| 1 | 1655.274 | 314.490 | 276.008 | 32 | 148486 | 148486 | 0 |
| 2 | 1612.416 | 317.284 | 277.043 | 32 | 148486 | 148486 | 0 |
| 3 | 1553.506 | 308.812 | 273.047 | 32 | 148486 | 148486 | 0 |
| 4 | 1582.657 | 313.681 | 278.859 | 32 | 148486 | 148486 | 0 |

## Experiment 1

The builder no longer keeps a vector per file. It counts records, calculates offsets and fills the flat slots directly. Focused inventory tests passed. The single build was 34.369 ms faster than the reference median, and peak RSS was 17.648 MiB lower. These observations are consistent with less transient storage, but one run cannot establish a speedup or attribute the difference to allocations. The schema, registrations, record count (148,486), parse count (148,486), failed parse count (0), worker count (32) and semantic index signature matched the reference.

## Experiment 2

Build-time scalar selection uses inline storage for one value, single scalar postings bypass the deduplication set, and validation paths borrow field names until an error needs text. Existing coverage and a focused selection test passed (110 unit tests passed before the focused test was added; the focused test also passed). The build was 42.849 ms faster than experiment 1, while peak RSS rose 2.117 MiB. The timing is a single observation; the memory change argues against claiming an allocation reduction from RSS alone. Schema, canonical registrations, counts, workers and semantic signature matched experiment 1 and the reference.

## Experiment 3

Built-in JSON readers omit owned text segments for build parsing. After schema conversion, the raw bigram builder visits logical strings by borrow, resetting gram history for each string. Reader-supplied segments remain in use for `ReaderSegments` readers. Full core tests and a focused temporal/nested-string gram parity test passed. The build was 58.908 ms faster than experiment 2, with peak RSS 3.945 MiB lower. This is a single timing observation, not a causal allocation measurement. Schema, canonical registrations, counts, workers and semantic signature matched all previous samples.

## Experiment 4

An ordinary file batch now reuses one read buffer, with an 8 MiB capacity retention ceiling. The oversized framed-file path is unchanged. Tests covered increasing and decreasing file sizes, empty files, malformed and healthy siblings, a missing file, a framing failure that takes ownership of bytes, and oversized capacity release. Full core tests passed: 113 unit tests passed, one ignored, plus one integration test. The build was 29.140 ms slower than experiment 3 and peak RSS rose 5.148 MiB. This single observation is within the 78.368 ms spread of the reference's three runs and cannot establish a regression or an allocation cause. The change remains provisional because it removes repeated buffer creation with limited production code. Schema, canonical registrations, counts, workers and semantic signature still matched.

No query, restore or profiling benchmark was run. The four post-change builds are the complete measurement budget for this experiment series.
