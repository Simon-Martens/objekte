# Query extensions: pre-feature baseline

The measured code is `e9d22a405c151dca1a3a8c5d87a39cb9de708db2`, the development HEAD selected before the query-extension branches. The release example was compiled before timing and run directly against a byte-for-byte copied Musenalm root at `/home/simon/source/objekte-query-bench-corpus`. This keeps benchmark snapshots outside the user's TUI corpus. The copy retained source metadata; the measured corpus fingerprint is `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`, and the schema manifest digest is `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`.

The run used Rust 1.99.1 release, the i9-13900K, `performance` governor, CPUs 0–31 affinity, 32 query workers, zero source/decoded-cache budgets, ASCII and Unicode alphabets, disabled and 64 MiB warm verified-cache profiles, and five timed repetitions per case/profile. The filesystem cache was not flushed. The full invocation and raw output are in `run-manifest.json` and `console.log`; machine and build flags are in `environment.json`. The two active feature workers deferred Cargo builds and indexing during the timed run.

All 150 existing requests completed in all four profiles: 600 query rows and 3,000 timed samples, with zero failures or cross-profile result mismatches. Their full per-query times, counts and work counters are in `query-results.csv`, `measurements.jsonl`, `query-plans.jsonl`, and `REPORT.md`. Earlier correctness verification on this exact code revision passed 300/300 jq oracle ID sets; the oracle was not rerun as part of this timed sample.

| Profile | Sum of 150 per-query median times, ms |
| --- | ---: |
| ASCII, verified cache disabled | 1,654.295 |
| ASCII, 64 MiB warm verified cache | 1.548 |
| Unicode, verified cache disabled | 1,606.305 |
| Unicode, 64 MiB warm verified cache | 1.570 |

One fresh construction was included per alphabet, with five fresh-process restore measurements in each case. ASCII reported 3.999 s build, 0.325 s snapshot write, 111.214 MB snapshot, 481.4 MB resident and 571.8 MB peak RSS. Unicode reported 1.187 s build, 0.331 s snapshot write, 113.725 MB snapshot, 712.9 MB resident and 780.4 MB peak RSS. The first alphabet ran before the second on the copied corpus; these single build samples do not establish an alphabet speed difference. Their raw phase and restore samples are in `index-results.json`.

This recording is the before state for TASK-10. It is not evidence that the new query extensions are fast or correct; final code requires its own full run and focused feature workload.
