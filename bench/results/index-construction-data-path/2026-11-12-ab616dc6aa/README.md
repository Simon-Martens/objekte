# Index construction data-path evidence

Task `index-construction-data-path`, 12 November 2026. Fixed baseline B is remote `master` `ab616dc6aabb9b9fee4657deb6752a6512bec9e5`. The selected construction merge, root-relative path and shared ID changes, and archived comparison cleanup are included in B. All experiments began at B. The original checkout stayed on `master`.

Host: Intel Core i9-13900K, 32 logical CPUs, Linux `7.2.15-200.fc44.x86_64`, `performance` governor, CPU affinity `0-31`, Rust `1.99.1`, locked dependencies. Musenalm has 148,486 files and 193,297,230 source bytes; inventory fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`; schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`.

Release examples were built with `cargo build --release --locked -p objekte-search-core --example bench_index_construction` in separate worktrees and target directories. The control binary SHA-256 is `d9f29ad155893c6e6d22c0c88a21ec114f44953e01c4b7551a769da9e34daeef`. Each comparison's `config.json` gives exact worktree paths, revisions, binary hashes, alphabet, worker count, and command inputs. `runs.jsonl` gives all warm-ups and measured trials; each trial holds the complete sample and run manifest. Timing runs have profiling off and run sequentially in fresh processes.

The comparison command template is:

```bash
python3 bench/results/index-construction-optimization/2026-11-12-master-737cfb77/compare.py \
  --control-repo CONTROL_REPO --candidate-repo CANDIDATE_REPO \
  --control-bin CONTROL_BIN --candidate-bin CANDIDATE_BIN \
  --root /home/simon/source/objekte/examples/musenalm \
  --alphabet ascii|unicode --pairs 3|5 --out OUTPUT_DIR
```

- `ascii-folding/`: baseline versus scalar-only `66c40c0edb`; `ascii-folding-kernel/`: scalar/reference/SIMD gram-emission measurements from the earlier SIMD candidate `d938fa5618`.
- `compact-contributions/`: baseline versus compact arena `ee5e176416`; `allocations/`: focused B/candidate DHAT evidence; `contribution-storage/`: separate opt-in full-corpus B/compact capacity profiles, with the temporary B instrumentation patch and its measurement boundary. Candidate instrumentation is pushed at `78c290475a`.
- `parallel-admission/`: baseline versus initial parallel candidate `f4e9c0013b` at 32, 1, and 4 workers; later gated candidate `3cecb58840` at 4, 8, and 16 workers; separate 4-worker opt-in overlap profile. Each run manifest identifies its exact revision.
- `combined/`: baseline versus scalar plus parallel integration `382ba91251`; `interaction/`: scalar-only versus that combined revision. These measurements led to rejecting parallel admission in the final code.
- `final/`: baseline versus final scalar-only integration code at `4f51c45e22`, with three pairs for each alphabet and an additional five-pair ASCII run after the three-pair result was inconclusive. The final binary SHA-256 is `42449be3ded342ff6228c88a52a4dddd200f6501314eff3da739803cb4be3c0b`.
- `profile/`: separate opt-in B/final component profiles. Their service durations are not normal timing trials.
- `snapshot/`: sample, run-manifest, sizes and hashes for baseline/final snapshots. The four 129–132 MB payloads remain in ignored `target/index-construction-data-path/snapshot/` and are omitted from Git. `restore-results.json` checks the five fixed queries; `restore-all-results.json` checks all 194 extended-manifest queries under final-code required-load mode. Both sets restored with zero build parses. `verify_index_construction_snapshot.rs` is the verifier source, temporarily copied into `crates/core/examples/` for compilation and removed afterward.
- `*.log.gz`: exact test, clippy, and formatting-check output. The repository-wide formatter fails on pre-existing drift, including on baseline B. The changed-code correctness checks and `git diff --check` pass.

The concise interpretation and retention decisions are in `docs/ai/generated/index_construction_data_path_results.md`.
