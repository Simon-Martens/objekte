# Native segment construction repair results

Task: `native-segment-construction-repair`

Integration branch: `integration/native-segment-construction-repair`

Implementation revision measured: `d6941e17598f922d745c589fc88c0004ea4003d0`

Immediate baseline: `integration/native-segment-construction` at
`9c61fbc5d5510ae1a2713ffba160d2155742abfe`.

Raw output is under `docs/ai/generated/native-segment-construction-repair/`.

Release executable SHA-256:

- `bench_build_musenalm`: `7b3a8467dc2a670d6993f12e16f0a95289cb20606f6d40f3cfe6bee2b7482631`
- `bench_musenalm`: `fa711828290629f2874eababfe10c99f6f59b17e30beb243411fba6415004a99`

## Construction

The final release binary was built with the locked workspace and run three times
per configuration on the 148,486-record Musenalm corpus. Values are medians.

| Configuration | Build time | Peak RSS | Segments | Parses | Peak concurrent segment jobs |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII, 1 worker | 3,606.1 ms | 480.2 MB | 15 | 148,486 | 1 |
| Unicode, 1 worker | 4,345.5 ms | 488.6 MB | 15 | 148,486 | 1 |
| ASCII, 8 workers | 800.8 ms | 520.1 MB | 15 | 148,486 | 9 |
| Unicode, 8 workers | 923.9 ms | 545.8 MB | 15 | 148,486 | 10 |

All runs had zero failed parses. ASCII logical signatures were identical across
workers and repetitions; Unicode signatures were likewise stable.

Against the immediate reviewed branch, the final ASCII eight-worker build is
0.6% slower and Unicode is 2.0% slower. Peak RSS is 3.3% higher in ASCII and
5.5% higher in Unicode. Both the time and memory gates pass.

The segment distribution remains nine `contents` segments and one segment each
for `agents`, `entries`, `items`, `pages`, `places`, and `series`.

## Queries

The unchanged 150-case workload ran in ASCII and Unicode with verified caching
disabled, four query workers, and three repetitions per case. Repository and
progressive Index each produced 300 successful case/profile rows.

| Entry point | Alphabet | Median ratio | p95 ratio | Mean ratio |
| --- | --- | ---: | ---: | ---: |
| Repository | combined | 1.007 | 1.223 | 1.023 |
| Progressive Index | combined | 1.003 | 1.161 | 1.022 |

Per-case status, total matches, returned rows, candidates, file opens, bytes,
parses, exact posting matches, scanned records, and residual evaluations are
identical to a fresh run of the immediate baseline on the same machine.

Two progressive rows exceeded the investigation threshold. Seven-repetition
isolated reruns measured `Q076` Unicode at 1.01x and `Q143` Unicode at 0.98x
baseline. The initial difference was run noise.

Final hybrid metrics from the benchmark's build phase:

| Alphabet | Complete build | First publication | Peak RSS |
| --- | ---: | ---: | ---: |
| ASCII | 757.6 ms | 469.8 ms | 687.7 MB |
| Unicode | 802.6 ms | 522.6 ms | 990.6 MB |

## Correctness regressions

- Repeated local slot zero in two relation source segments returns only the
  matching logical source.
- A relation target found at local slot zero in a later target segment is not
  redirected to local slot zero in the first segment.
- `@collection.name = @record.id` and its reverse evaluate per record, including
  inequality and Boolean combinations.
- Inferred numeric paging merges current multi-segment orders and remains
  source-free after an addition and after retiring a leading segment.
- Adopted initial segment payloads are reclaimed after replacement plus
  compaction while the Index remains alive; a pinned old view keeps its payload
  until released.
- One 9 MiB JSONL file constructs more than the scheduler window's worth of
  segments and publishes all records.
- A collection-wide inference minimum reached only across segments admits the
  field globally, with exact posting results and no source parses.
- Declared postings now reduce as independent owned field jobs on the shared
  Rayon pool.

## Validation

- `cargo test -p objekte-search-core --lib --locked`: 528 passed, 16 ignored.
- `cargo test -p objekte-search-core --tests --locked`: all integration tests
  passed.
- `cargo test --workspace --locked --no-fail-fast`: passed across CLI, desktop,
  core, integration tests, and doctests.
- `cargo check --workspace --all-targets --locked`: passed.
- `cargo check -p objekte-search-core --all-targets --features
  detailed-profiling --locked`: passed.
- `cargo clippy --workspace --all-targets --locked`: passed with existing
  warning-level diagnostics.
- `git diff --check`: passed.

## Remaining limitations

- The current compatibility sort metadata retains locale/options but no copied
  segment keys.
- Declared field postings are scheduled independently; custom posting/text
  families still reduce within their family jobs rather than one task per key.
- Independent field reductions increased peak RSS beyond the 10% engineering
  gate. Correctness, complete-build time, and query work are preserved.
- The aggregate query p95 ratio is elevated by long broad-text cases and
  cross-run noise. The two progressive threshold hits above 20% and 1 ms were
  not reproducible.
