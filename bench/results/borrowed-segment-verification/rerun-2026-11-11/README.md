# Borrowed segment verification rerun archive

This is the 11 November 2026 same-computer rerun of predecessor `5166876c7228` against follow-up `f872e8d750fd`. The [analysis report](../../../../docs/ai/generated/borrowed_segment_verification_rerun_2026-11-11.md) explains the results and limitations.

- `focused-run-commands.json`, `recheck-run-commands.json`, and `in-memory-run-commands.json` record exact focused commands, worktrees, and wall times. Each corresponding JSONL file contains the benchmark's setup row and per-query timing samples, allocation counts, membership digest, and work counters. The `recheck` files use 21 repetitions; other focused files use seven.
- `focused-comparison.csv` pairs all 80 focused query rows and checks ordered membership and source-work equality.
- `suite-run-commands.json` records the four serial full-suite commands. The `base-suite-*` and `final-suite-*` directories contain their run manifests, full timing samples, query summaries, build measurements, plans, schema manifests, and query definitions. Each suite log records the console output.
- Synthetic fixture definitions and digests are in [`../fixtures/`](../fixtures/). The fixtures were regenerated at `/home/simon/source/objekte-bench-borrowed-segment-verification/` with the documented commands in [`../README.md`](../README.md); all five hashes matched.

No result-cache reuse was enabled in the focused runs or cache-off suite. The warm suite used 64 MiB, as its manifests record. All runs were serial and used four query workers.
