# Allocation and timer separation: Musenalm FTS4

This run compares production baseline `266d9a0fb071838be8f757671599872a7036a054`
with extension prototype `a59fd665ffd7067580e97c3c708b48a845dbad8a`.
The reference worktree is based on `2d6d876ee7` because that commit adds only
benchmark harness and evidence to the baseline production source. Both sparse
worktrees received the identical benchmark-only `harness.patch` (SHA-256 in
`manifest.json`). The profiler source files have identical hashes. Each of the
eight binaries was built with its own Cargo target directory. The candidate
lockfile differs only by the local `objekte-extension-tests` package entry;
all dependency versions used by the profiler are the same.

Both binaries read the original Musenalm corpus at the same absolute path.
The content, source metadata and schema fingerprint matched the saved Phase 0
fingerprint before and after measurement. The profile built 133,948 `contents`
records with ASCII bigrams and the four original FTS field overrides. Every
timed run used `--skip-snapshot`; the measured interval is exactly
`FilePicker::profile_collection`. Filesystem page caches were warm. One
untimed warm-up per binary preceded five alternating reference/candidate pairs
for each configuration. Timed processes ran serially. CPUs `0-15` are the
host's eight performance cores and their SMT siblings; the one-worker runs
used CPU `0`. The explicit Rayon worker counts were 16 or 1.

| Mode | Allocator counting | Stage timers | Workers | Reference median (ms) | Candidate median (ms) | Delta (ms) | Delta |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| A | off | off | 1 | 8133.4 | 7941.3 | −192.2 | −2.4% |
| A | off | off | 16 | 1551.0 | 1517.1 | −33.8 | −2.2% |
| B | off | on | 1 | 8281.4 | 8477.2 | +195.8 | +2.4% |
| B | off | on | 16 | 1580.8 | 1567.8 | −13.0 | −0.8% |
| C | on | off | 1 | 8196.8 | 8407.2 | +210.4 | +2.6% |
| C | on | off | 16 | 3058.5 | 2768.7 | −289.8 | −9.5% |
| D | on | on | 1 | 8637.1 | 8644.7 | +7.6 | +0.1% |
| D | on | on | 16 | 3038.6 | 2888.8 | −149.8 | −4.9% |

Clippy required a source-only adjustment after this matrix: the disabled
allocator path now returns a zero-sized token instead of `()`. The final patch
and binary digests are in `harness-final.patch` and `manifest-final.json`;
the preceding matrix retains its original patch and binary identities. Mode A
was repeated with the final binaries and the same warm-up and pair protocol:

| Final mode A | Reference median (ms) | Candidate median (ms) | Delta (ms) | Delta |
| --- | ---: | ---: | ---: | ---: |
| 1 worker | 8176.2 | 7905.7 | −270.5 | −3.3% |
| 16 workers | 1539.5 | 1518.6 | −20.9 | −1.4% |

The final A16 pairs had candidate-minus-reference differences of +8.1,
−18.9, −30.6, −34.1 and −19.2 ms. The 8.1 ms slower pair is below the
reference-only spread. The final A1 pairs were all faster for the candidate.

`summary.json` and twelve raw process records per mode/worker directory
preserve the full samples, order, commands, exits, worker counts and work
counters. All 96 processes completed successfully, returned 133,948 records
and 47,456,174 compressed index bytes. Allocation and stage metrics are
`null` when their instrumentation is disabled, rather than reported as zero.
The timer-enabled runs reported the requested Rayon worker count and identical
per-field index sizes across both revisions. No source corpus or schema file
changed during the experiment.

The same reference executable was also compared with itself in five paired
runs. A16 had a median difference of −2.8 ms, with pair deltas −17.8 to
+9.5 ms. A1 had −27.1 ms median difference and pair deltas −185.0 to
+138.6 ms. D16 had −14.9 ms median difference and pair deltas −223.9 to
+292.0 ms. That D16 range is comparable to the earlier apparent candidate
slowdown. The old D16 series was +10.6% candidate; the new one is −4.9%,
so the direction does not reproduce under the matched construction boundary.

The production-allocator, no-timer A configuration has no reproducible
candidate slowdown at either worker count, including with the final binaries.
At 16 workers, enabling allocation counting adds
roughly 1.2–1.5 seconds to this workload; at one worker its added cost is
much smaller. This supports an instrumentation interaction that scales with
parallelism, but does not establish a specific cache-line or scheduling cause.
B1 and C1 each show a candidate slowdown above 100 ms, so those diagnostic
configurations remain recorded as distinct findings. They are absent in A.
The original instrumented elapsed-time gap is therefore not evidence of a
production-allocator latency regression in this matched workload.

## Index correctness and signature limits

The timer-enabled full-snapshot validation ran a complete build, save and
restore for both revisions. It reported identical document counts, component
sizes, compressed index bytes, and base/secondary snapshot sizes. Its raw
snapshot byte hashes differed. A repeated build with the *same reference
binary* also produced different hashes at 16 workers, so a parallel snapshot
byte hash is not a stable correctness signature here.

At one worker, three complete B-mode builds (reference, candidate, reference)
produced identical base snapshot SHA-256
`3664fcb4acae44cab3f857f3301213730942067ef3c562b0a98729b02274abd6`
and identical secondary SHA-256
`68b908b4fc881648cd5d696b49a4c525a26e4211f32ad507aab2035b850e2cf0`.
Each restored snapshot passed the profiler's index-size equality check.
`serial-signatures.json` and the three raw result files retain the evidence.
The failed 16-worker byte-equality attempt is retained in `signatures.json`
and `validate_signatures.py`; it was not discarded.

## Gate A assessment

The matched production-like construction comparison supports no reproducible
default-path regression on this FTS4 workload. The counting allocator and
timers are compile-time independent, and the normal allocator installs no
wrapper or per-allocation flag check. Workspace tests and Clippy passed with
four Cargo build jobs. The spacing check still reports formatting differences
in previously committed WIP files and the baseline bigram file; it reports
none in the changed profiler. The original slowdown remains an
instrumented-profile observation with substantial reference-versus-reference
variance. The mechanism behind that variance and the B1/C1 diagnostic
slowdowns is not established. This assessment is limited to this workload;
it does not clear the later text-integration, query or full-matrix gates.

No CLI latency measurement was used for this recovery comparison. CPU
temperature was unavailable; the instantaneous frequency samples in the
manifest were taken after the runs and do not establish per-run clocks.
