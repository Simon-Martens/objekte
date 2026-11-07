# Inference retention experiment

These are single release-build samples from 2026-11-07. The baseline is code
`267946c29aeba1a2e3cf5976c843dce61bdec76e`; the final measurement is
code `183d96ffad0a55243e8bc5300805073ba763a742`. The small benchmark
fixture has two source versions because the final code exposes additional
build-only counters: use `bench_inference_retention_baseline.rs` at the baseline
revision and `bench_inference_retention.rs` at the final revision. Both prepare
and open the same data.

## Reproduction

- Rust `1.99.1` (`58cf8f9c3`, 2026-10-13), Cargo release profile, no recorded
  `RUSTFLAGS`, target override, or release profile override.
- Fedora Linux 44, kernel `7.2.13-200.fc44.x86_64`, Intel Core i9-13900K,
  32 logical CPUs. The two build invocations ran separately in uncontended
  windows. CPU affinity and filesystem cache were not fixed.
- One collection, ASCII bigrams, `InferenceMode::AutoIndex` with the default
  policy, four build workers, snapshot mode disabled. The fixture creates
  1,024 JSONL files with 32 logical records each (1,638,400 source bytes).
  The `manual` field has a declared posting, `blocked` has an explicit
  automatic-index opt-out, and `kind` is a repeating inferred string field.
- The SHA-256 of the sorted file-checksum list is in `fixture.sha256`. The
  fixture was prepared once before the baseline timing and reused unchanged.

From the repository root at each code revision:

```sh
cp bench/results/library-improvements/inference-retention/bench_inference_retention_baseline.rs \
  crates/core/examples/bench_inference_retention.rs
cargo build --release -p objekte-search-core --example bench_inference_retention
./target/release/examples/bench_inference_retention prepare
./target/release/examples/bench_inference_retention measure > baseline.json

# At the final code revision, with the same generated corpus:
cp bench/results/library-improvements/inference-retention/bench_inference_retention.rs \
  crates/core/examples/bench_inference_retention.rs
cargo build --release -p objekte-search-core --example bench_inference_retention
./target/release/examples/bench_inference_retention measure > final.json
```

`prepare` is outside the timed interval. The `measure` timer covers repository
opening with snapshots disabled; compilation, JSON output, hashing and process
teardown are outside it. The raw JSON also records the library's internal
build, discovery, extraction/index, finalization and coordinator admission
phases. Admission is a component of extraction/index and must not be added to
the other phases. Observation time alone was not measured: per-record timers
would distort this fast workload. Inference merge and finish times are
available only in the final sample.

## Results and decision

| Measure | Baseline | Final | Final minus baseline |
| --- | ---: | ---: | ---: |
| Repository build (ms) | 66.078 | 53.037 | -13.041 |
| Extraction/index (ms) | 53.516 | 43.644 | -9.872 |
| Coordinator admission (ms) | 20.081 | 15.470 | -4.611 |
| Finalization (ms) | 8.631 | 6.739 | -1.892 |
| Peak process RSS (KiB) | 36,352 | 30,280 | -6,072 |

The 19.7% build-time decrease is one focused observation, not a repeatable
speedup estimate. Both samples parsed 32,768 records with zero failures,
admitted the same one inferred index, and had the same inferred-index SHA-256:
`67fdd17b60441a021152d961451f6e374d96600b847a668ac9eb861f42deb604`.
The published index coverage and admission outcome therefore match in this
fixture.

The report intentionally changes: baseline `manual` and `blocked` summaries
each counted 32,768 distinct short strings. The final report marks those two
summaries incomplete, reports the unknown count as zero, and gives an explicit
bound-exceeded reason. Their `Redundant` and `ExplicitOptOut` index statuses
remain. The inferred `kind` report and index are unchanged.

Final build-only counters show 32,768 observed records, two merged batches,
2.623 ms inference merge, 0.016 ms inference finish, 2,652,867 bytes of
cumulative policy charge, 785 bytes of unique path metadata estimate, 1,570
bytes of repeated batch path charges, and zero retained manual-summary bytes.
These are estimates and operation counters, not allocator counts or an RSS
limit. Baseline equivalents of the new counters are unavailable.

Prompt worker cancellation after inference becomes unpublishable was deferred.
Normal and framed workers can be in flight when ordered coordinator admission
discovers the failure; cancellation requires a deterministic boundary and
explicitly ignoring later partial inference contributions while ordinary
parsing and indexing continue. No timing-dependent cancellation was added.
