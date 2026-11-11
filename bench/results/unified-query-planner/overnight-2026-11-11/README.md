# Unified planner benchmark provenance

The full acceptance runs are `baseline-suite` (engine commit `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`) and `matched-suite` (engine commit `05c00a3cbfa2f218ed063681eccbd9769386aef5`). The `final-suite` directory is an earlier preliminary run before the Q189 sort guard. Focused `planning-*` runs use the identical benchmark harness source, built with `--release --locked`; `baseline-*` binaries contain the engine at `6ef66ec8`, and `final-*` binaries contain the engine at `00f50190` (which differs from `05c00a3c` only by the benchmark harness option and documentation). All runs used the same absolute corpus root, four query workers and the cache sizes recorded in their manifests.

The `flagged-*`, `deep-*`, `isolated-*`, and `warm190-*` rechecks invoked saved exact-revision binaries from the current checkout. For those runs, `run-manifest.json::code_revision` records the *checkout used to launch the binary*, not the binary's engine revision. Names containing `base` or `baseline` use `6ef66ec8`; names containing `matched` use `05c00a3c`. Their result summaries and companion CSV/JSONL files are unmodified. The binary files and build/test logs are local reproduction artifacts and are not committed.

See `docs/ai/generated/unified_query_planner_results.md` for interpretation, thresholds, and the remaining measurement limits.
