# Q4 computed-sort benchmark

This directory holds a standalone harness for the local and related computed-sort paths. The harness is compiled against an explicit checkout, so the same `main.rs` and manifest template measure both the base and changed code. It never writes to the original `master` checkout.

## Protocol

From this task worktree, build once for each checkout before timing:

```sh
bench/results/library-improvements/computed-sorting/build.sh CHECKOUT
```

Then run each mode without invoking Cargo:

```sh
Q4_ROWS=256 Q4_REPS=7 bench/results/library-improvements/computed-sorting/run.sh CHECKOUT OUTPUT-local.jsonl local
Q4_ROWS=256 Q4_REPS=7 bench/results/library-improvements/computed-sorting/run.sh CHECKOUT OUTPUT-related.jsonl related
```

The default profile is `release`. `Q4_TARGET_DIR` can select an explicit per-checkout build directory. The default is `CHECKOUT/target/q4-computed-sorting`. The build script places its scratch Cargo package there, and the runner creates and removes fixtures under its `fixtures` directory. The runner refuses a binary built for a different checkout head or a different `main.rs` hash.

Each mode measures both 256 small source files and one 256-record JSONL file with identical logical records and stable intrinsic IDs. Local sorting uses a case-folded title; related sorting uses a target title. Related runs use eight targets for high reuse and 256 for low reuse. Every query requests 32 byte payloads after an offset of 16 and a complete count, with verified-result caching disabled and snapshots disabled. The runner checks final hit parity against the reference path after timing.

The JSONL output records fixture creation and repository open costs separately from query latency. The first query has `phase=first`; seven later queries have `phase=repeat`. The ordered-hit SHA-256 includes count, selected identities, relative source locations, evidence, and byte payloads; hashing happens after the timed query interval. Output also records target reads and reuse, source bytes, retained key estimates, and retained payload estimates. Compare medians and spread of repeated samples; small differences near run-to-run variation should be treated as noise. These runs use warm filesystem cache after fixture creation and do not represent cold-disk latency.

At 256 records the fixture SHA-256 values are:

| Mode | Layout | Reuse | SHA-256 |
| --- | --- | --- | --- |
| local | many | none | `01ce45a81d0d9d2fcb5eafe8607f6e65866287181ea6cb5e4d626f38a933eb13` |
| local | one | none | `1013af0233c5ef28060edcd15d874c256f6e52e43deff1ac6f50d628e71c53e1` |
| related | many | high | `d4172a0399cce1d593b2696f96c76a796cdddc425b1016e0e5bf9baa958ef533` |
| related | many | low | `d810d0f32f664f4b11d1b3fa2e8e761b8e352f1d977d15a43131054a7a552edd` |
| related | one | high | `9f5bc100df2c5aea89ed62026e9854a23f92b89eff6c79f6d2440db3b37b4e5c` |
| related | one | low | `eab9c924c87d4cc865c5adc960f685ee66581a7556addb9c8b692781265666ad` |

Files prefixed `preliminary-no-digest` lack ordered-hit hashes. Files prefixed `preliminary-generated-id` used generated IDs containing source modification times, so their hashes cannot establish cross-checkout parity. These preliminary runs are excluded from the official comparison.

## Commit-1 result

The official comparison is immutable base `ca54a873f97efa3aea23a16b4d52bb0f0274510e` against code commit `9b27a2f50c614ebee7b4a71b5f5f1b9aecbfcd1d`. Run `analyze.py baseline-local.jsonl final-local.jsonl baseline-related.jsonl final-related.jsonl` to recheck all 24 paired cases and see the complete median and min/max table in `summary.txt`. The analyzer verifies every fixture hash, ordered answer hash, count, and target-read/reuse count. Each query also passed the reference-route hit comparison in the harness.

| Local source layout | Workers | Base repeat median (ms) | Commit-1 repeat median (ms) | Ratio |
| --- | ---: | ---: | ---: | ---: |
| Many files | 1 | 0.896 | 0.869 | 1.03× |
| Many files | 2 | 1.579 | 1.870 | 0.84× |
| Many files | 4 | 3.189 | 0.928 | 3.44× |
| Many files | 8 | 1.978 | 0.600 | 3.30× |
| One JSONL | 1 | 0.534 | 0.454 | 1.18× |
| One JSONL | 2 | 0.496 | 0.442 | 1.12× |
| One JSONL | 4 | 0.472 | 0.458 | 1.03× |
| One JSONL | 8 | 0.491 | 0.454 | 1.08× |

The 2-worker many-file samples overlap substantially: base 1.494–2.178 ms, final 1.448–2.660 ms. This case is inconclusive. The one-JSONL differences are small absolute changes on submillisecond queries and do not show a file-concurrency gain. Related sorts still use one file per group in commit 1, so the mixed related timings in `summary.txt` do not support a target-lock claim. High target reuse remained 8 reads and 248 reuses; low reuse remained 256 reads and no reuse.

The scan-retained payload estimate fell from at most 203,056 bytes to zero because page payloads are loaded after ranking. This counter excludes the source read batch and final page materialization, so it is not process peak memory. For the many-file, eight-worker local case, source bytes read rose from 1,060,864 to 1,193,504 as selected page payloads were read again. Median repository-open time across local cases was 22.433 ms at base and 21.101 ms at commit 1; these build times are separate from query timing.

## Commit-2 result

The separate cache comparison is commit-1 evidence head `773e26fe4573427406ce9a31288fd4fc0b2949f8` against code commit `2b6fd5b7bdc506674783082215c6f2f82f6165b3`. Both use the same source harness and 256-record fixtures. Run `analyze.py commit1-related.jsonl commit2-related.jsonl` to validate all 16 paired cases and reproduce `summary-commit2.txt`.

| Related source layout | Reuse | Workers | Commit-1 repeat median (ms) | Commit-2 repeat median (ms) | Ratio |
| --- | --- | ---: | ---: | ---: | ---: |
| Many files | High | 1 | 0.918 | 0.897 | 1.02× |
| Many files | High | 2 | 1.589 | 0.890 | 1.79× |
| Many files | High | 4 | 2.531 | 0.602 | 4.20× |
| Many files | High | 8 | 2.144 | 0.871 | 2.46× |
| Many files | Low | 1 | 1.397 | 1.361 | 1.03× |
| Many files | Low | 2 | 2.618 | 1.198 | 2.18× |
| Many files | Low | 4 | 3.204 | 1.340 | 2.39× |
| Many files | Low | 8 | 2.687 | 1.161 | 2.31× |

One JSONL file cannot be split between these file workers; its paired ratios ranged from 0.97× to 1.04×. High reuse retained exactly eight target reads and 248 reuses; low reuse retained 256 reads and zero reuses. The cache reserves each target's estimated bytes before loading and retains a 64 MiB target-cache cap. Worker reservations leave target-cache headroom within the 128 MiB computed-sort budget. These estimates and benchmark counters do not measure process peak RSS. Median repository-open time was 22.308 ms for commit 1 and 21.968 ms for commit 2; query times exclude opening and fixture generation.
