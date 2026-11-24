# Performance: small-file add and move exceed the paired lifecycle gate

- **Category:** `perf`
- **Originating task:** `record-ingestion-and-file-catalog`
- **Branch and measured implementation:** `integration/record-ingestion-and-file-catalog` at `55f6bb8ba596f1503d232f86a67a58a233c441b4`
- **Base:** `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`
- **Code paths:** `crates/core/src/lifecycle/mutation/preparation.rs::prepare_add_file`, `crates/core/src/lifecycle/mutation/mod.rs::add_file` and `::try_content_preserving_move`, catalog replacement and publication in `crates/core/src/files/` and `crates/core/src/lifecycle/`

## Observed and expected

The final-SHA three-pair interleaved confirmation of `bench_lifecycle_mutations small` produced these process medians (the example reports one phase duration per process):

| Phase | Base process samples, ms | Final process samples, ms | Median base → final | Difference | Spec verdict |
| --- | --- | --- | ---: | ---: | --- |
| Add | 1.668, 1.766, 0.290 | 1.898, 2.158, 2.159 | 1.668 → 2.158 ms | +0.490 ms, +29.38% | **Breach** |
| Move | 0.052, 0.084, 0.054 | 0.062, 0.076, 0.065 | 0.054 → 0.065 ms | +0.011 ms, +20.37% | **Breach** |

The task's section 10.3 allows a lifecycle phase increase of no more than 10% **or** 5 µs. Each observed increase exceeds both limits. It requires three alternating interleaved base/final process pairs and says a confirmed breach blocks completion. The initial final-SHA three-run matrix had add +0.026 ms (+6.1%) and move −0.017 ms (−27.9%); the confirmation is the specified deciding measurement. Earlier candidate confirmations were also variable, including a three-pair add recheck that cleared the gate on an earlier SHA. None replaces the final-SHA result.

The expected behavior is the same correct add/move lifecycle with confirmed median phase costs within the specified gate. All other paired gates and 27 native API after-only runs pass, including the canonical logical results, the zero-source-read count, and all 30 confirmed historical screening QIDs.

## Reproduction and evidence

Use the existing baseline and integration worktrees and binaries, with the same host settings described in [`summary.md`](../../generated/record-ingestion-and-file-catalog/summary.md). The standalone confirmation driver is [`run_confirmations.py`](../../generated/record-ingestion-and-file-catalog/run_confirmations.py):

```sh
python3 docs/ai/generated/record-ingestion-and-file-catalog/run_confirmations.py \
  --baseline-bin /home/simon/source/objekte-baseline-record-ingestion-and-file-catalog/target/release/examples \
  --after-bin /home/simon/source/objekte-integration-record-ingestion-and-file-catalog/target/release/examples \
  --fixture /home/simon/source/objekte/examples/musenalm \
  --baseline-worktree /home/simon/source/objekte-baseline-record-ingestion-and-file-catalog \
  --after-worktree /home/simon/source/objekte-integration-record-ingestion-and-file-catalog \
  --out /home/simon/source/objekte-integration-record-ingestion-and-file-catalog/docs/ai/generated/record-ingestion-and-file-catalog/confirmation-recheck \
  --phases lifecycle-small
```

The `--out` path must be new and disposable for a recheck. `OBJEKTE_BENCH_MUTATE_COPY=1` is set by the driver. The raw final observations, commands, binary hashes, exit status and checksums are under [`confirmation/`](../../generated/record-ingestion-and-file-catalog/confirmation/); side-by-side metrics are in [`comparison.csv`](../../generated/record-ingestion-and-file-catalog/comparison.csv). All six final confirmation invocations exited zero. The driver first reads the fixed Musenalm JSON files outside the timed lifecycle process, alternates first-run side, and creates a fresh small mutation fixture per invocation.

## Impact and follow-up

The regression affects small file-backed add and move phase latency under this workload. It does not indicate a logical result failure. The implementation already contains catalog copy-on-write shards and a file-only record count route; prior versions were measured and revised. The add path prepares and indexes the file outside publication locks; move hashes the destination and publishes a catalog replacement. Those are code-path facts, **not** a diagnosis of which operation accounts for the measured delta. Process and filesystem variability is evident from the separate runs, especially the 0.290 ms base add sample, but the mandated confirmation still fails. No further safe targeted optimization was established before the final measurement, and accepting a confirmed slowdown without approval would violate the task specification.

Two concrete next steps are available for user review:

1. **Request more optimization and a fresh final-SHA matrix.** Profile add and move at subphase granularity, change the implementation if justified, rerun workspace checks and the complete Stage 7 matrix and confirmation on the new SHA. This retains the existing performance gate.
2. **Grant an explicit exception for these two measured lifecycle phases on SHA `55f6bb8ba596f1503d232f86a67a58a233c441b4`.** Record the exception in the task report and complete the workflow without claiming these gates passed. This accepts the measured latency for the specified workload only.

The task remains blocked pending that choice. This issue is for user review and does not itself authorize a separate task.
