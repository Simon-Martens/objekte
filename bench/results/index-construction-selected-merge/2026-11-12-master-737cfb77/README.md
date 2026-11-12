# Selected index construction merge: acceptance evidence

Control: remote `master` `737cfb7745ff15915804553ab583af260ac0f2e4`.
Candidate: `integration/index-construction-selected-merge` code revision
`e212e4ad5f2a691507540395d11947398b45ef36` (A1 reverted).

`ascii-auto/` and `unicode-auto/` contain the driver configuration, every
warm-up and measured run, raw sample and manifest, and paired summaries.
`profile/` is one separate opt-in candidate run. `snapshot-*/` and
`queries-*/` contain correctness samples and manifests; `restore-*.json`
contains the archived verifier's `SnapshotMode::Required` results. The large
snapshot payloads and release binaries are omitted. Paths embedded in raw
files point to the original ignored `target/index-construction-selected-merge/measurements/`
location; replace that prefix with this archive to inspect the copied files.

Both release examples were built with
`cargo build --release --locked -p objekte-search-core --example bench_index_construction`,
using separate `CARGO_TARGET_DIR` values under
`/home/simon/source/objekte/target/index-construction-selected-merge/{control,candidate}`.
The binary SHA-256 hashes are in each `config.json`.

The ASCII timing command was:

```bash
python3 bench/results/index-construction-optimization/2026-11-12-master-737cfb77/compare.py \
  --control-repo /home/simon/source/objekte-task-index-construction-selected-merge-control \
  --candidate-repo /home/simon/source/objekte-integration-index-construction-selected-merge \
  --control-bin /home/simon/source/objekte/target/index-construction-selected-merge/control/release/examples/bench_index_construction \
  --candidate-bin /home/simon/source/objekte/target/index-construction-selected-merge/candidate/release/examples/bench_index_construction \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii --pairs 3 \
  --out /home/simon/source/objekte/target/index-construction-selected-merge/measurements/ascii-auto
```

The Unicode command changed `--alphabet` to `unicode` and the output suffix
to `unicode-auto`. The driver ran one warm-up per role and alternated order
for three pairs. There was no overlapping compilation, test or benchmark.
All timing runs had profiling disabled and used 16 resolved workers. For the
profile run, the candidate example received `--profile --alphabet ascii`.
The same binary was invoked directly with `--root
/home/simon/source/objekte/examples/musenalm --result-dir
/home/simon/source/objekte/target/index-construction-selected-merge/measurements/profile`.
For correctness runs, each binary was invoked directly with the same `--root`,
`--check-queries`, `--alphabet ascii` or `unicode`, and the matching
`--result-dir` under `measurements/{snapshot,queries}-{control,candidate}-*`;
ASCII also received `--snapshot`. The temporary verifier source is preserved in the
older archive as `verify_index_construction_snapshot.rs`. Each generated
`index.snapshot` was made available to it as `search-core-ascii.snapshot`;
both restored with zero build parses. Snapshot sizes were 129,139,458 bytes.
SHA-256: control `7bedf4d2b54eab4ff1dac108ac226b4dd2340746f019aad9002e66e70fbce6e1`,
candidate `36dc873d160ec744c7c2e826138c55ce735ce8cde5f8970a3a7911cc02dc7095`.

The configured corpus and schema fingerprints, environment, counters,
timings, memory samples and interpretation are in
[`docs/ai/generated/index_construction_selected_merge_results.md`](../../../../docs/ai/generated/index_construction_selected_merge_results.md).
