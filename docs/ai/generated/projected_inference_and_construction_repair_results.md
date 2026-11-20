# Projected inference and construction repair results

Task: `projected-inference-and-construction-review`

Base revision: `3f0c9f9f6c453e950a7414bae8748481c2c6225a`

Final code revision for these measurements: `b80e5fad87`

Branch: `repair/path-posting-query-evaluation`

## Correctness

The projected inference tests now compare compacted reports with fresh builds of
the surviving records for both reported counterexamples. The terminal-scalar
fixture produces one scalar record and zero terminal-array records after both
compaction and consolidation. The integer fixture produces a complete report
with `integer_only = Some(true)` after compaction and consolidation. Retiring
all numeric records leaves no retained integer-only claim.

The projected-report validator also rejects scalar, array, object, empty-array,
empty-selection, or nonblank record counts that exceed the surviving bounds.
The existing public inference validation still runs in each regression test.

Commands:

```text
cargo test -p objekte-search-core --lib --locked projected_ -- --nocapture
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --all-targets --locked
cargo check -p objekte-search-core --all-targets --features detailed-profiling --locked
cargo clippy --workspace --all-targets --locked
cargo fmt --all -- --check
git diff --check
```

Results:

- Seven selected projected-inference tests passed.
- The full workspace suite passed. Core ran 652 library tests with 16 ignored;
  all CLI, desktop, integration, and documentation tests passed.
- Workspace and detailed-profiling checks passed.
- Clippy completed without errors and retained the repository's existing
  warning-level diagnostics. The warnings introduced in the touched code were
  removed.
- Formatting and whitespace checks passed.

## Construction environment and protocol

- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Records: 148,486 JSON source files in seven collections
- Alphabet: ASCII
- Requested construction workers: 8
- Verified cache: disabled
- OS: Fedora 44, kernel `7.3.3-200.fc44.x86_64`
- Rust/Cargo: `1.99.1`
- CPU: Intel Core i9-13900K, 32 logical CPUs
- CPU governor: `performance`

Separate ordinary release binaries were built from the base and candidate
revisions. The corpus was warmed before measurement. Five fresh-process pairs
alternated binary order. Ordinary wall and peak-RSS measurements came from
`bench_index_construction`; profiling was a separate, single run per role and
configuration.

Ordinary binary SHA-256:

- Baseline: `718db93ce99f93768b76c352d517663ea7056083f357b6e4204c264ee7ce5cb9`
- Candidate: `f00fceaf985d3054f200c5448c2de93b905e32d53da0ee67d9e5e8124737eabc`

## First batch transfer

All 15 segment jobs used the zero-base transfer path. The profile counters
recorded 15 transfers, 19,565 transferred field/length keys, and 410,403
bitmap members whose replay was avoided.

Instrumented sampled segment work:

| Configuration | Aggregate merge, baseline | Aggregate merge, candidate | Ratio |
| --- | ---: | ---: | ---: |
| Inference disabled | 996.881 ms | 915.033 ms | 0.918 |
| AutoIndex | 1,960.790 ms | 1,808.533 ms | 0.922 |

These are sampled segment-job totals. Jobs overlap, so these values are
comparison counters and must not be added to wall time.

Fresh-process end-to-end build:

| Configuration | Baseline median | Candidate median | Ratio | Candidate peak RSS |
| --- | ---: | ---: | ---: | ---: |
| Inference disabled | 887.696 ms | 887.385 ms | 1.000 | 542.9 MiB |
| AutoIndex | 1,595.941 ms | 1,561.403 ms | 0.978 | 869.2 MiB |

Inference-disabled peak RSS was 533.6 MiB at baseline and 542.9 MiB at the
candidate, a 1.7% increase. AutoIndex peak RSS was 894.9 MiB at baseline and
869.2 MiB at the candidate, a 2.9% decrease. All ten measured runs parsed
148,486 records with zero failed parses and identical posting-field, sort-column,
and relation-edge counts as reported by the benchmark.

Inference-disabled build samples, in milliseconds:

| Pair | Baseline | Candidate |
| ---: | ---: | ---: |
| 1 | 887.696 | 869.404 |
| 2 | 858.599 | 912.264 |
| 3 | 904.700 | 850.364 |
| 4 | 863.609 | 901.867 |
| 5 | 888.038 | 887.385 |

AutoIndex build samples, in milliseconds:

| Pair | Baseline | Candidate |
| ---: | ---: | ---: |
| 1 | 1,613.609 | 1,547.706 |
| 2 | 1,586.977 | 1,561.403 |
| 3 | 1,640.135 | 1,537.595 |
| 4 | 1,595.941 | 1,674.028 |
| 5 | 1,591.916 | 1,589.547 |

The end-to-end inference-disabled result is neutral within run noise. The
AutoIndex direction is favorable, but the distribution contains a candidate
outlier and is not used as a regression gate.

## Declared posting populations

`bench_posting_populations` ran in seven fresh processes per revision. Each
process reports the median of five in-process samples. The table records the
median of those seven process-level medians.

Command:

```text
target/release/examples/bench_posting_populations
```

| Population | Records | Baseline | Candidate | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Singleton scalar fields | 50,000 | 15.882 ms | 14.229 ms | 0.896 |
| Three-element array | 25,000 | 11.033 ms | 9.191 ms | 0.833 |
| Repeated object-array prefix | 5,000 | 17.766 ms | 17.861 ms | 1.005 |
| 32 duplicate strings per array | 10,000 | 9.089 ms | 9.190 ms | 1.011 |

The direct insertion path benefits short arrays. The repeated-prefix and
duplicate-heavy workloads remain effectively unchanged, so the temporary-set
fallback has not been replaced for general multivalue selections.

Ordinary population binary SHA-256:

- Baseline: `f507579beb4e0638850eef282eadbbb8089d6a2a947256093d1cfafe93d7fd1b`
- Candidate: `cb31f37d6a3131d52ceda813f24850e358c7bc9825a25b8f18148e25c85fd360`

## AutoIndex observation

The AutoIndex profiler measured 808.905 ms of sampled inference-finish work at
baseline and 664.152 ms at the candidate, a ratio of 0.821. The ordinary
end-to-end build median fell from 1,595.941 ms to 1,561.403 ms and peak RSS fell
from 894.9 MiB to 869.2 MiB. This isolates the avoided transient vector clone
without claiming that the overlapping sampled interval is wall time.

## Reproduction note

The profile values above came from `bench_index_construction --profile
--incremental`, because its incremental reporting path emits the per-segment
profile without the ordinary benchmark's strict nested interval reconciliation.
The latter rejects summed overlapping extraction intervals in both revisions.
The profile run is explanatory only; all end-to-end comparisons used separate
uninstrumented binaries and fresh processes.

Raw process output and result directories were retained under
`/tmp/objekte-review-timing-20261120-2052` for this run. The exact per-process
medians and configuration needed to reproduce the comparisons are recorded
above.
