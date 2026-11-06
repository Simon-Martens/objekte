# Musenalm regression checks

Run these commands from the repository root. Each command writes a separate result directory under `bench/results/musenalm/`; pass `--result-dir` to choose another directory.

| Check | Command | What it verifies |
|---|---|---|
| Independent oracle | `python3 bench/verify_jq_oracle.py --result-dir bench/results/musenalm/jq-oracle-local` | `jq` evaluates 150 explicit predicates over all 148,486 original JSON files. The core returns complete ID sets under ASCII and Unicode. All 300 sets must agree. Pass `--save-membership` to retain full gzip membership files; per-case SHA-256 digests are always saved. |
| Fast iteration | `python3 bench/run_query_regressions.py quick` | One cold ASCII run of all 150 cases. Checks sample count, status, exact count against the independent `jq` baseline, page size against the pinned baseline, corpus/schema/workload fingerprints, and latency budgets. |
| Full query gate | `python3 bench/run_query_regressions.py full` | Five repetitions under each of ASCII cold, ASCII warmed, Unicode cold, and Unicode warmed: 3,000 samples. The same checks apply with tighter latency budgets. |
| Index builds | `python3 bench/run_build_regressions.py` | Three fresh processes for each of ASCII/Unicode with snapshots disabled/rebuilt. Checks complete single-parse builds and compares wall time, peak RSS, and snapshot size to `bench/baselines/musenalm-build.json`. |
| Archived core | `python3 bench/report_old_core_comparison.py --result-dir bench/results/musenalm/old-core-local` | Loads both cores with the Musenalm schemas, warms the archived indexes, disables verified query caches, and compares exact counts and ordered page IDs before timing 34 shared structured queries. |

The query latency gate uses [`final-8dd0c2aa6d`](results/musenalm/final-8dd0c2aa6d/) as its pinned baseline. It enforces timing budgets on the same CPU model and reports ratios elsewhere. The `jq` count baseline is [`musenalm-jq-counts.json`](baselines/musenalm-jq-counts.json). The index build baseline is created once with `python3 bench/run_build_regressions.py --establish-baseline` and then kept in Git. Fresh result directories are required because benchmark measurement files are created exclusively.

The archived comparison uses record ID ascending order, exact count, and 50 logical records for both libraries. It measures only cases whose complete counts and ordered page IDs match; its report records excluded cases and source work. The two cores have different cache and index internals, so read the work counters alongside timing ratios.
