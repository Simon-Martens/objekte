# Native B-tree memory-only restoration: acceptance evidence

Task: `restore-native-btrees-memory-only` review completion.
Branch: `integration/restore-native-btrees-memory-only`.
Reviewed head: `6184a89e1954f80bd7a489288a9b9bb6bd08e241`.
Final code commit: `2a2c12189d5a4a48ff644d6cf744c6a837b7d3d5`.
Ledger addendum commit: `4078a3c5d65446b413ac343a2701316e3c66097a`.

All runs used the same host, release builds unless noted, 32 available CPUs, and the
immutable Musenalm corpus at
`/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm`
(148,486 records, 859 MB). Verified-result reuse was disabled for the query campaign.

## Query campaign

Baseline binary: `bench_musenalm-start`
`sha256 76416b5238ba28e98ad6be107868677ef3e7898d4da9d42e126f3abb7de82c61`
(START `66b932f31f922a13062a61658e843e3033b3aff4`).

Candidate binary: `bench_musenalm-accepted`
`sha256 dd06568cf941545e639049fe0cea479cc7c479cc438df064ea433f10ea9a1e53`
(code `2a2c12189d5a4a48ff644d6cf744c6a837b7d3d5`).

```sh
BASELINE --root "$CORPUS" --extended --incremental \
  --profiles ascii-off,unicode-off --verified-cache-bytes 0 \
  --query-workers 32 --repetitions 5 --storage memory \
  --result-dir query/baseline

CANDIDATE --root "$CORPUS" --extended --incremental \
  --profiles ascii-off,unicode-off --verified-cache-bytes 0 \
  --query-workers 32 --repetitions 5 \
  --result-dir query/candidate
```

Result directories above are the resolved `query/baseline` and `query/candidate`
directories in this evidence package. `measurements.jsonl.gz` retains the raw
per-repetition samples; `REPORT.md`, `query-results.csv`, `result-digests.json`,
`index-results.json` and `run-manifest.json` are the benchmark outputs.

Summary: 440/440 `(case, profile)` rows matched status, total count, returned
count and full 220/220 result digests. Candidate/START median timing ratio
**0.922**, geometric mean **0.857**. 256 cases were >5% faster, 138 within ±5%,
46 >5% slower (mostly microsecond-scale; the previously investigated Q172
outlier was 0.72×/1.01× on nine-repetition isolated reruns). ASCII/Unicode
incremental open was 1.401/1.593 s → 1.094/1.034 s; process RSS was
568.3/1049.4 MiB → 541.7/1033.2 MiB.

## Fresh-build gate

```sh
python3 bench/run_build_regressions.py --binary "$BUILD_CANDIDATE" \
  --result-dir build/candidate
python3 bench/run_build_regressions.py --binary "$BUILD_START" \
  --result-dir build/baseline
```

Candidate `bench_build_musenalm-accepted`:
`sha256 99067ea4aa1387203ed7659c917183bb9d565a3390e358ba09593aca92b0db09`.
START: `sha256 baae76a9889e6c721138fcdf0ff2fc132024f4273c464a9a4e923b0d9082806f`.

Three repetitions: START ASCII 0.962 s / peak RSS 460.7 MiB, Unicode
0.985 s / 484.1 MiB; candidate ASCII 0.919 s / 465.7 MiB, Unicode
0.969 s / 480.1 MiB. All parses 148,486, zero failures.

## Independent jq oracle

```sh
CARGO_TARGET_DIR="$TARGET" python3 bench/verify_jq_oracle.py --root "$CORPUS" \
  --result-dir jq-oracle
```

150 predicates × ASCII/Unicode: 0 membership mismatches. `jq-oracle/REPORT.md`
and `run-manifest.json` are retained.

## Lifecycle and concurrency

`lifecycle/mutation-start.txt` and `mutation-candidate.txt` are the disposable
small-fixture campaigns. Candidate completed add 0.171 ms, append update
0.072 ms, verified update 0.070 ms, move 0.032 ms, verified delete 0.020 ms,
compaction 0.000 ms and no-change reconciliation 0.103 ms; START stopped at its
removed managed-flush phase after the native phases.

`concurrency-*.json` records START and candidate initial-build concurrency:
build wall time improved from 1.199→0.969 s (idle), 1.143→0.980 s (foreground
queries) and 1.118→1.025 s (held view).

## GUI status polling and frontend checks

The follow-up regression test proves repeated unchanged status polling performs
zero `index_family_estimate` calls; adding a file recomputes the new segment
estimate and polling then reads only cached totals. The production GUI was
exercised in headless Sway with a configured 200,000-record / 100-file JSONL
repository at `/tmp/opencode/restore-btrees/gui-corpus-big`:

- idle CPU with the Index panel closed: 1.4% before-review binary, 1.6%
  candidate debug binary (10 s `/proc/<pid>/stat` utime+stime samples);
- idle CPU with the Index panel open: 1.4% before, 1.6% candidate.
  GUI repaint dominates this idle measurement; the removed traversal is proven
  by the instrumentation test and code path, not by a measured CPU delta.
- Index panel showed Searchable readiness, source freshness, revision 4,
  `Estimated index memory: 5.5 MiB`, 100 files/200,000 records, Rebuild/Compact.
- File check examined 100 files with 0 changes; Compact reported 0 obsolete
  slots; cancellation stopped a broad query and left the app responsive.
- The application menu and Index panel contain no storage open/save/retry
  actions. The small-corpus FTS run returned two highlighted matches.
- Screenshots: `frontend/gui-index-panel.png`, `gui-large-corpus.png`,
  `gui-check.png`, `gui-compact.png`, `gui-menu-no-storage.png`,
  `gui-query-small-corpus.png`.

PTY TUI evidence in `frontend/tui-results.txt` and `tui-actions.txt` shows
progressive construction, two FTS matches, and the action menu without storage
controls.

## No-index-I/O matrix

`no-io/query-*` is the CLI open/query matrix on a fixture containing
`.objekte/.indexes/old/manifest.toml`, `.objekte/indexes/old/segment.idx`,
`.objekte/search-core-ascii.snapshot`, `cache/objekte-indexes/old.idx` and a
`alias-index` symlink into `.objekte/indexes`. `strace` recorded no access to
any generated namespace; the fixture inventory (paths, sizes, mtimes) was
unchanged and the query returned two hits.

`no-io/lifecycle-*` performs add, update, move, delete, compaction, rebuild,
reconciliation and shutdown. `strace` again recorded no generated-namespace
access; only the intended source files changed. The fixture used the same
malformed/inaccessible artifacts plus the symlink alias.

The CLI process tests additionally cover absent, malformed and inaccessible old
artifacts with identical two-hit source results.

## Warnings and formatting

Clippy actual diagnostics (excluding summary lines): 299 at START, 221 at the
final candidate. Comparing by file and diagnostic message after normalizing the
three "large size difference ... at least N bytes" messages shows **zero new
diagnostic categories**; the three size messages changed size only. The review
fixes also removed all newly introduced needless-borrow/unnecessary-cast
warnings in the native accessors.

`cargo fmt --all -- --check` reports the repository's pre-existing global drift
(79 files) at both START and the final tree; no unrelated reformat was applied.
`git diff --check` is clean.
