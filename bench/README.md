# Musenalm regression checks

Run these commands from the repository root. Each command writes a separate result directory under `bench/results/musenalm/`; pass `--result-dir` to choose another directory.

For focused planning comparisons, `bench_musenalm` accepts `--cases Q035,Q141` and `--repetitions 7`. `--verified-cache-bytes 0` disables verified-result reuse.

| Check | Command | What it verifies |
|---|---|---|
| Independent oracle | `python3 bench/verify_jq_oracle.py --result-dir bench/results/musenalm/jq-oracle-local` | `jq` evaluates 150 explicit predicates over all 148,486 original JSON files. The core returns complete ID sets under ASCII and Unicode. All 300 sets must agree. Pass `--save-membership` to retain full gzip membership files; per-case SHA-256 digests are always saved. |
| Fast iteration | `python3 bench/run_query_regressions.py quick --comparison repair --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19` | One uncached ASCII run of all 150 cases. Checks sample count, status, exact count against the independent `jq` baseline, page size against the selected baseline, resolved workload/schema, corpus identity, environment, and latency budgets. |
| Full query gate | `python3 bench/run_query_regressions.py full --comparison repair --baseline bench/results/musenalm/controlled-foundation-performance-92cdb92b19` | Five repetitions under each of ASCII uncached, ASCII warmed, Unicode uncached, and Unicode warmed: 3,000 samples. The same checks apply with tighter latency budgets. |
| Index builds | `python3 bench/run_build_regressions.py --result-dir bench/results/musenalm/build-local` | Three fresh processes for each of ASCII/Unicode. Checks complete single-parse builds and records wall time and peak RSS. Pass `--baseline` with a prior `gate.json` from this script for a comparable regression check. |

Query gates select a named comparison and explicit baseline. The repair comparison uses the immediate pre-repair foundation revision `92cdb92b19`; historical recordings remain in `bench/results/musenalm/`. Compare build results only with a baseline recorded by the current build script, which no longer measures legacy snapshot writes. Keep CPU policy and affinity comparable when interpreting timings. The `jq` count baseline is [`musenalm-jq-counts.json`](baselines/musenalm-jq-counts.json). Fresh result directories are required because benchmark measurement files are created exclusively.
