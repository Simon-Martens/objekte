# Opt-in full-corpus contribution storage counters

This compares baseline B `ab616dc6aa` with compact candidate `78c290475a` on the same configured Musenalm corpus, 16 workers, ASCII and Unicode. The profiles are separate from ordinary timing runs. Both use the same opt-in sampling logic; `baseline-instrumentation.patch.gz` decompresses to the temporary, uncommitted B-only source change. B was restored clean afterward. Candidate instrumentation is committed on its experiment branch. B's profile manifests therefore show three modified files, while candidate manifests are clean. The patch is archived to make that difference reproducible.

Build commands in the respective worktrees:

```bash
CARGO_TARGET_DIR=target/profile-instrumented cargo build --release --locked -p objekte-search-core --example bench_index_construction
CARGO_TARGET_DIR=target/profile-contrib cargo build --release --locked -p objekte-search-core --example bench_index_construction
```

The first command built instrumented B; the second built the compact branch. The binaries' SHA-256 hashes were `7c6da4965a2097e0876674134765385d5a65ff0b41d53f2f6eb0ac71a90eaf47` and `d2a3a836f8bdbbbd6f4c84c3f606eb610e47f8bbafb08980b0f639fcf4476f97` respectively. Each binary ran with `--root /home/simon/source/objekte/examples/musenalm --alphabet <ascii|unicode> --workers 16 --profile --result-dir <role>/<alphabet>`; all four sample and run-manifest files are archived here. Corpus and schema fingerprints matched the main measurement archive.

| Alphabet | Role | `size_of` block / descriptor / column store / contribution | Final global blocks / columns | Final global element-capacity bytes | Peak admitted batch element bytes | Peak received-ready element bytes | Process HWM RSS |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | B | 16 / 24 / 48 / 104 B | 6,807,376 / 25,477 | 175,316,640 B | 821,824 B | 18,560,768 B | 543,432,704 B |
| ASCII | Compact | 16 / 8 / 72 / 152 B | 6,807,376 / 25,477 | 209,148,240 B | 798,720 B | 19,223,040 B | 467,976,192 B |
| Unicode | B | 16 / 24 / 48 / 104 B | 7,248,099 / 37,909 | 186,578,224 B | 988,736 B | 23,156,096 B | 546,750,464 B |
| Unicode | Compact | 16 / 8 / 72 / 152 B | 7,248,099 / 37,909 | 222,791,568 B | 798,720 B | 19,554,816 B | 473,931,776 B |

The global value is the observed maximum and final value in these builds. It counts block vector capacity and hash-table key/value element capacity, but excludes hash control bytes, allocator rounding, and the struct fields shown separately above. The flat arena holds more element capacity than B despite its smaller descriptor. B has many separately allocated vectors, so this element-capacity lower bound cannot explain or predict process RSS; the lower compact RSS is an independent observation. The 10,000-record DHAT fixture in `../allocations/` also has a different peak-live result and does not represent the full corpus.

Admission samples include all pending collections and the current batch before and after merge. The received-ready queue is sampled separately. Active workers and completed results not yet read from the channel are excluded, and the two reported peaks may occur at different instants. These are lower bounds on contribution-owned heap elements, not a complete peak-live byte total. Final persistent index estimates were identical across roles: 69,662,968 B ASCII and 72,571,368 B Unicode. The full release comparisons in `../compact-contributions/` found a 34–39% build regression, so the candidate was rejected regardless of its lower process RSS.
