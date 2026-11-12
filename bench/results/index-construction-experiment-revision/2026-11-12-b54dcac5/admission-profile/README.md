# Field-admission scheduling profiles

Separate opt-in builds compare clean B `54dcac5c3b` with field-admission candidate `b38d6696e4`. Both use the canonical Musenalm root, release `bench_index_construction`, `--profile`, and explicit `--workers 32` for both alphabets or `--workers 16` for the Unicode regression case. These are single diagnostic runs, excluded from ordinary paired medians. The ordinary timing evidence is under `../field-admission/`.

Run each role's release example with `--root /home/simon/source/objekte/examples/musenalm --alphabet ascii|unicode --workers 32|16 --profile --result-dir OUTPUT_DIR`. Per-run manifests record hashes, revisions, CPU and corpus identity. All runs have zero failed parses and matching final counters.

| Case | Grouped batches / total | Posting / text groups | Peak concurrent groups | Group overlap | Submission-to-start sum | Preparation/submission | Group execution sum | Merge/join interval |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 32 workers ASCII | 9 / 293 | 9 / 9 | 2 | 8.114 ms | 0.199 ms | 1.677 ms | 18.239 ms | 10.970 ms |
| 32 workers Unicode | 9 / 293 | 9 / 9 | 2 | 9.333 ms | 0.326 ms | 2.049 ms | 21.486 ms | 12.830 ms |
| 16 workers Unicode | 9 / 293 | 9 / 9 | 2 | 8.264 ms | 0.444 ms | 1.451 ms | 20.534 ms | 12.549 ms |

All grouped batches belonged to `agents` (eight) or `series` (one); `contents`, the largest collection, had none. The gate's coarse work estimate did not find two substantial independent groups in its batches. The candidate therefore demonstrated real overlap but applied it to only a small part of this corpus. Ready-to-admission delay sums were 10.049 s ASCII / 10.170 s Unicode at 32 workers and 4.419 s Unicode at 16 workers; these are queue-residence sums across batches, not recoverable wall time. Ordered record admission remained on the coordinator (78.9–83.5 ms summed at 32 workers). Remaining sequential structural/sort work after the scoped merge was 0.218 ms ASCII / 0.227 ms Unicode at 32 workers and 0.137 ms at 16 workers.

The `merge_join_ns` interval starts after the last group submission and ends when the scope joins. It includes coordinator inference/relation/custom-posting work done while groups execute. Group execution intervals are elapsed service durations; their sum may overlap and is not CPU time. The overlap value comes from the actual group start/end interval sweep. Different profile samples can disagree with ordinary paired timing, so these intervals explain scheduling rather than establish a speedup.
