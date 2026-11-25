# Repair concurrent ingestion of large JSONL files

Implement this repair on `integration/concurrent-jsonl-file-ingestion`, starting from `integration/external-json-loaders` at `8c58b8dedd38eeb555cc1501688f43ee054d4c61`. Use a sibling worktree under `/home/simon/source` and follow `docs/ai/README.md`. Push the repair branch for review. Merging into master is a separate decision.

## Defect

The shipped `objekte_loader_jsonl::register` calls `register_file_batched` in `crates/loader-jsonl/src/reader.rs`. In `crates/core/src/construction/initial.rs`, large files from that registration become `ScheduledBuildJob::Large`. The coordinator finishes and publishes one large file before starting the next. Chunks from one file can decode in parallel, so a high decoder count can coexist with completely serial processing across files.

The existing `several_large_jsonl_files_preserve_one_pass_locators_and_publication` test checks the final index with one and four workers. It would pass with the current serial file schedule.

## Required behavior

1. With multiple workers and two independently publishable large JSONL files, one file must be able to frame, decode and publish while the other is paused in framing. A single large file must still be able to use multiple decoder workers. A one-worker build must finish.
2. Keep small-file batching. Use the existing reader capabilities, file classification, repository worker pool and shared streaming budget. Inspect `run_stream_groups`, `process_framed_file_with_budget` and the pull-framing path before adding scheduling machinery. The callback path currently receives `peer_files`; the pull-framing path does not. Ensure one file cannot reserve all the capacity its peer needs to progress.
3. Preserve one normal content pass per file, correct record IDs and byte ranges, and atomic publication of each file revision. A file bound to multiple collections must remain atomic across its bindings. On cancellation or fatal error, stop dispatching, drain active work and release reservations before reporting completion. Earlier published files remain searchable.

## Decisive regression

Use `Index::start_build` with the actual `objekte_loader_jsonl::register` registration. Create two files larger than 8 MiB, each containing several chunks, and set `max_files = 1` so they can publish independently. Use four workers.

At a test-only framing or input-read boundary, hold whichever file enters first, before it reaches EOF. Let the other file continue. While the gate is still closed, require a pinned view to find a known record from the other file and zero records from the held file. Then release the gate, wait for completion, and verify both files’ counts and representative first, middle and last byte ranges.

Use synchronization events and a bounded timeout. Release the gate and join the build on every outcome so the regression fails cleanly on the parent. The test instrumentation must leave production registration and scheduling decisions intact.

Run the same data with one worker and the gate disabled. Also run more large files than workers to exercise saturation. Retain the existing cancellation, failure-draining, blank-frame and single-file parallel decode regressions.

## Validation and delivery

Run formatting, the locked workspace check and tests, and the release `external_loaders` integration suite. Compare parent and candidate on the same host using the shipped loader for three workloads: one large JSONL file, several large JSONL files, and many small JSONL files.

Use short paired release runs. Record worker count, median build time, peak RSS, peak concurrently active files, peak decoder jobs, and content opens and bytes read. Investigate any repeatable material slowdown. The gated visibility test decides correctness independently of machine-specific speedup.

Append the parent and final SHAs, the gated result on each revision, and the focused measurements to the completion report. Commit the evidence with the repair branch.

## Implementation report

- Task `concurrent-jsonl-file-ingestion`
- Completed 2026-11-25
- Branch `integration/concurrent-jsonl-file-ingestion`
- Review worktree `/home/simon/source/objekte-integration-concurrent-jsonl-file-ingestion`
- Parent `8c58b8dedd38eeb555cc1501688f43ee054d4c61`
- First implementation commit `b48af41f2c8f52a532bd0dd6d612c25f32f41ece`
- Final implementation commit `623444e66d6d0935e8748037036701dabbc308fa`

### Scheduling and publication

The initial classifier still batches ordinary files and uses the existing single-file pipeline for isolated pull-framed files. Within a multi-worker build, independently publishable large pull-framed files use the existing scoped stream-group scheduler. Each is a one-file publication group. A scoped set of up to four file coordinators feeds the repository's existing decoder pool. The grouping keeps a file's bindings private until preparation has completed for all of them. Large bindings that use the same registered reader reuse each framed input chunk and fingerprint in one content pass. Bindings that use different readers retain their distinct framing and prepare within the same atomic file publication.

The stream pipeline uses a per-file share of the shared job and byte allowance. Completion releases shared input capacity as soon as a decoder frees the chunk, so a framer paused before its next read cannot pin capacity with completed work. Completed contributions remain charged to their file's local ordered window. Cancellation interrupts reservation waits and is checked at the pull input boundary. Fatal framing and decoding results stop that file's dispatch. Both pipelines drain active decoder work before the group reports the failure or cancellation.

### Gated release regression

The fixture uses `Index::start_build` with `objekte_loader_jsonl::register`, four workers, `max_files = 1`, and two files of 9,000 records each. Each file is larger than 8 MiB. The test-only hook stops the first file before framing its first chunk. The observer waits on publication events and checks a pinned view for the peer's known record and an empty record set for the held file. It releases the gate and waits for build completion on every result.

| Revision | Result with the gate closed |
| --- | --- |
| Parent `8c58b8dedd38eeb555cc1501688f43ee054d4c61` plus the test hook and fixture | Peer publication timed out after five seconds. The test released the gate and waited for the build, then failed with the expected visibility assertion. |
| Final implementation | The peer published while the first framer was held. The held file had zero visible records. Both counts and the first, middle and last ranges matched after release. |

The release suite also verified the same two files with one worker, five large files with four workers, and the two-file gate with both files bound to two collections. It verified a single content pass and atomic visibility across both collection bindings. A cancellation after the peer published left that peer searchable in the terminal partial view.

- `cargo fmt --all -- --check` passed.
- `cargo check --locked --workspace --all-targets` passed.
- `cargo test --locked --workspace` passed 1,086 tests with 21 ignored. The retained regressions passed for cancellation, fatal-error draining, zero-record chunks and single-file parallel decoding.
- `cargo test --locked --release -p objekte-cli --no-default-features --test external_loaders` passed all six tests.

### Paired release benchmark

Intel Core i9-13900K with 32 logical CPUs on Linux and Rust 1.100.0. The parent and repair built the same benchmark harness separately with `cargo build --locked --release -p objekte-cli --no-default-features --features detailed-profiling --example bench_jsonl_file_concurrency`. The parent received only the test and measurement instrumentation. Both executable copies used the shipped registration and the same prepared bytes. The table shows medians from five fresh-process pairs, alternating the order of the parent and candidate. Timing covers index startup to build completion. Peak RSS comes from Linux `VmHWM`. Peak active files and decoder jobs come from the stream counters for framed reads and the active-file extraction probe for normal batches.

Run `bench_jsonl_file_concurrency ROOT <one|several|small> <1|4|32> --prepare` once per workload, then run the same command without `--prepare` to measure. Workloads contain one file with 100,000 records, five files with 36,000 records each, and 4,096 small files with four records each. The first two keep `max_files = 1`, and the small-file workload uses up to 64 files per segment.

| Workload | Workers | Median build ms | Median peak RSS MiB | Peak active files | Peak decoder jobs | Content opens | Content bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| One large file | 1 | 260.85 → 246.45 | 55.5 → 55.7 | 1 → 1 | 1 → 1 | 1 | 28,800,000 |
| One large file | 4 | 84.43 → 84.81 | 56.2 → 56.1 | 1 → 1 | 4 → 4 | 1 | 28,800,000 |
| One large file | 32 | 63.81 → 62.78 | 63.6 → 63.9 | 1 → 1 | 18 → 16 | 1 | 28,800,000 |
| Five large files | 1 | 471.38 → 463.29 | 100.3 → 100.2 | 1 → 1 | 1 → 1 | 5 | 51,840,000 |
| Five large files | 4 | 172.45 → 167.28 | 101.5 → 104.3 | 1 → 4 | 4 → 4 | 5 | 51,840,000 |
| Five large files | 32 | 131.82 → 107.72 | 109.4 → 113.5 | 1 → 4 | 19 → 31 | 5 | 51,840,000 |
| Many small files | 1 | 47.07 → 47.45 | 23.2 → 23.4 | 1 → 1 | 1 → 1 | 4,096 | 1,048,576 |
| Many small files | 4 | 30.14 → 30.61 | 23.7 → 23.8 | 4 → 4 | 4 → 4 | 4,096 | 1,048,576 |
| Many small files | 32 | 30.37 → 31.86 | 30.4 → 30.3 | 31 → 31 | 31 → 31 | 4,096 | 1,048,576 |

The five-file build reached four simultaneous file coordinators at both multi-worker settings. At 32 workers it reduced median build time by 18.3% with 4.1 MiB more peak RSS. The single-file build kept multiple decode jobs, and all workloads preserved their content opens and bytes read. The initial 32-worker small-file median increased from 30.37 to 31.86 ms. Nine additional alternating pairs measured 31.91 to 30.17 ms, with overlapping run ranges. The small-file timing difference did not repeat.

[Paired release samples](../../../bench/results/concurrent-jsonl-file-ingestion/paired-release.csv) and [32-worker small-file recheck](../../../bench/results/concurrent-jsonl-file-ingestion/paired-small-32-recheck.csv).

The repair worktree is retained for review. No subordinate worktrees were created.
