# Retained integration validation

At retained construction code `ceebc48811` (later commits in this branch add evidence and documentation only), `cargo test --workspace --locked` passed 820 tests, zero failed, 18 ignored across 21 result groups. `cargo clippy --workspace --all-targets --locked` exited successfully with warnings already present at B, including the unchanged `bigram.rs` `serialized_size() as usize` cast. Exact output is in the compressed logs here.

The repository's `scripts/format-rust-spacing.py --check` and `cargo fmt --all -- --check` both exited 1 on clean B and on this integration worktree. Their first reported drift is the same unchanged `crates/cli/src/execution.rs` import order. Baseline and candidate logs are archived separately; no unrelated formatting cleanup was made. `git diff --check 54dcac5c3b3e1ca499180061ebcee7ad60c27101...HEAD` passed.
