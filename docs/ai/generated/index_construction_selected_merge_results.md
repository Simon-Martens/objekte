# Selected index construction merge results

Task `index-construction-selected-merge`, 12 November 2026. Current remote
`master` control was `737cfb7745ff15915804553ab583af260ac0f2e4`.
Reviewed source head was `7a84a368152ce018fc1e8cd0e297032bf8c0ea02`.
Commit `5f03fb71a9f5dbcaf8914deac7ced83d3d3ec800` reverted A1, and
`e212e4ad5f2a691507540395d11947398b45ef36` added the focused parser
regression cases. The latter is the exact tested and measured implementation
revision. This report records integration readiness; the final `master` push
is verified separately after the merge.

## Selection and code review

The candidate retains M bounded parallel metadata preparation with a
64-path-per-worker window and ordered inventory insertion; E1 bounded
source-batch buffer reuse; E4 direct construction JSON deserialization into
existing logical values; A0 opt-in admission diagnostics; and the unused
admission-wrapper cleanup. A1 dispatch replenishment after each individual
admission is absent after its revert. E2 worker-local bigram scratch and E3
inline field selection are absent from the effective code. The reverted A1
commit remains in ancestry as experiment history. Remote `master` had not
advanced beyond the recorded baseline, so no master integration commit was
needed before testing.

The existing tests cover ordered metadata identity, shared memberships,
revision and first-error behavior; bounded buffer ownership and malformed
fallback; ordered admission and limits. The parser differential now also
covers the literal `$serde_json::private::Number` key, number boundaries and
a small exponent, in addition to existing duplicate-key, Unicode, invalid
input, array and depth cases. It compares results or diagnostics with the
old reader. The private serde_json number representation remains tied to the
locked dependency and should be checked on dependency updates.

## Short acceptance measurement

The host was an AMD Ryzen 7 5700G, 16 logical CPUs, Fedora kernel
`7.2.13-200.fc44.x86_64`, Rust `1.99.1`, `powersave` CPU governor.
The configured `examples/musenalm` corpus contained 148,486 files and
193,297,230 source bytes. Corpus fingerprint:
`db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c`;
schema fingerprint:
`4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`.
Both builds resolved 16 workers. Profiling was off for timing. Each role had
one warm-up followed by three alternating, fresh-process pairs per alphabet.
The existing `compare.py` driver and `bench_index_construction` example were
used with `--locked` release builds in separate target directories. The exact
driver command, per-run manifests and raw samples are in the
[acceptance evidence](../../../bench/results/index-construction-selected-merge/2026-11-12-master-737cfb77/README.md).
Control binary SHA-256:
`791485dbc791c548950708c14271c7cbe21195d986332baba7b4f9a7d8096b0e`;
candidate binary:
`367dc93efe248b70bc57286fc1ed185162a43894fb6402ef457d627e81c404d1`.

| Alphabet | Control median build | Candidate median build | Paired changes, candidate minus control | Paired median | Median peak HWM RSS, control → candidate |
| --- | ---: | ---: | --- | ---: | ---: |
| ASCII | 2,674.582 ms | 2,293.362 ms | −421.792, −398.201, −303.292 ms | **−398.201 ms (−14.888%)** | 544.8 → 552.4 MiB |
| Unicode | 2,993.612 ms | 2,439.168 ms | −662.951, −479.777, −477.151 ms | **−479.777 ms (−16.361%)** | 567.0 → 556.0 MiB |

The ASCII paired range was −421.792 to −303.292 ms; Unicode was −662.951
to −477.151 ms. Peak HWM RSS ranges were 542.2–544.9 MiB control and
539.0–555.0 MiB candidate for ASCII, 555.2–568.4 MiB control and
540.8–562.3 MiB candidate for Unicode. The small, inconsistent memory
differences across three pairs do not establish an A1-specific memory
saving. The former A1-inclusive combination measured 2,107.469 ms ASCII
and 2,255.591 ms Unicode in a separate five-pair run; those figures and
its controls are historical and are not combined with this acceptance run.

One separate profiled candidate ASCII build reported 16 peak active
metadata workers and admission component output: 1,242.205 ms summed
admission work, including 538.963 ms bigram contributions and 482.592 ms
structured postings. Scheduling reported 264 job-limit blocks, zero
byte-limit blocks and two prefetched later-collection jobs. Its aggregate
completion-to-receipt and receipt-to-admission intervals overlap; they
cannot be treated as recoverable elapsed time. This profiled run was excluded
from the medians.

## Correctness, snapshots and checks

`cargo test --workspace --locked` passed: 811 passed, zero failed, 17
ignored across 21 test groups. `cargo clippy --workspace --all-targets
--locked` passed with the established workspace warnings. The focused
direct-parser differential test passed after adding six cases. The changed
test file was run through rustfmt and the repository spacing formatter, and
`git diff --check` passed. The repository-wide spacing `--check` still
reports many unrelated, pre-existing formatting differences; this task did
not modify those files.

Fresh ASCII and Unicode control/candidate builds gave identical digests for
Q001, Q108, Q116, Q120 and Q194, zero failed parses, 148,486 build parses,
300 posting fields, eight sort fields and 228,503 relation edges. Estimated
index bytes matched control and candidate within each alphabet: 69,662,968
ASCII and 72,571,368 Unicode. Both ASCII snapshots were 129,139,458 bytes.
Using the archived verifier with selected code, both baseline-produced and
candidate-produced snapshots opened in `SnapshotMode::Required` with zero
build parses, `restored=true` and the same five query digests. Snapshot
hashes differed; byte identity is not part of the persistence contract.

The timing result is warm-cache and specific to this host and configured
corpus. There is no observed correctness or construction-time regression in
this acceptance run. Further admission work remains serial and is outside
the selected merge. The previous benchmark archive and its A1-inclusive
measurements remain unchanged.
