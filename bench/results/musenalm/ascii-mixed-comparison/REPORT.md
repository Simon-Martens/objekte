# C26 ASCII mixed Unicode literal comparison

The query change at `012f6fd518` retains every safe printable ASCII normal or
skip gram inside a mixed Unicode literal. The final scanner still verifies the
original literal. Before this change, any non-ASCII byte disabled the entire
ASCII bigram restriction.

The [before](../ascii-mixed-before-efb0c73860/REPORT.md) and
[after](../ascii-mixed-after-012f6fd518/REPORT.md) directories contain their
run manifests, exact queries, 25 raw samples each, query tables, plans, and
build/restore measurements. They used the same fixture/schema/workload
fingerprints, release compiler, hardware, cache-disabled ASCII profile, and
five repetitions per case in persistent processes. The runs were sequential.
The before revision is `efb0c73860`; the after revision is `012f6fd518`.

| Case | Before median (min–max) | After median (min–max) | Change | Candidates before → after | Parses before → after |
|---|---:|---:|---:|---:|---:|
| Q002 ASCII control | 185.482 ms (184.556–190.957) | 185.285 ms (184.391–192.148) | −0.197 ms | 6,645 → 6,645 | 6,645 → 6,645 |
| Q006 single `ü` | 2,463.339 ms (2,453.813–2,478.700) | 2,465.299 ms (2,447.394–2,514.999) | +1.960 ms | 148,486 → 148,486 | 148,486 → 148,486 |
| Q007 single `a` | 2,577.800 ms (2,556.150–2,594.786) | 2,579.110 ms (2,572.588–2,600.287) | +1.309 ms | 148,486 → 148,486 | 148,486 → 148,486 |
| Q013 `Müller` | 298.875 ms (298.244–301.162) | 244.523 ms (244.276–246.956) | −54.352 ms | 4,010 → 2,143 | 4,010 → 2,143 |
| Q024 `Frühling` | 1,943.550 ms (1,929.597–1,977.418) | 72.028 ms (70.720–72.864) | −1,871.522 ms | 133,948 → 4,636 | 133,948 → 4,636 |

The match and returned-page counts are unchanged for every case. Q013 and Q024
also matched the independent reference evaluator after the change. A literal
with no safe ASCII gram, such as `ü`, continues to scan conservatively.

This change affects query gram selection only. The one-run full ASCII build
was 14.197 s before and 14.329 s after; the five-run fresh-process restore
medians were 0.544 s and 0.530 s. Resident size after construction was
241.30 and 241.19 MiB. These small build/restore differences are not attributed
to the query change.

Reproduce each run with the arguments in its `run-manifest.json`, using
`cargo run --release --example bench_musenalm -- ...` from the repository root.
The `--result-dir` may be changed to a fresh location because the runner
creates `measurements.jsonl` exclusively.
