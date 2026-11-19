# Objekte · Assignment 2 verification addendum

**Merge decision.** Hold the Assignment 2 checkpoint for a correction to the verification tools. The repaired locator and reader tests cover the important production cases. The genuine checked-in inputs reproduce the published comparison byte for byte. The outstanding defects affect how that evidence is tested, extracted and reproduced.

| Reviewed item | Revision |
|---|---|
| Accepted A1 baseline | `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee` |
| Rust source tested by the agent | `d07b700a7188efd6b45ec542f197b78a211cb88a` |
| Delivery reviewed | `cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca` |

Continue on `integration/core-refactoring-completion`. The files below are in `docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/`.

## 1. Make every verifier self-test reach its intended check

`run_self_tests` writes `a1.json` and `candidate.json`. The child process opens `a1-reads.json` and `candidate-reads.json`. All five negative cases currently succeed as tests because the child encounters the same missing input file. I reproduced this under `python3 -O`. With the expected filenames, the actual verifier rejected paired empty, missing, duplicate and unknown queries, an incorrect candidate count, an omitted counter, reordered IDs and an altered trace offset. [Verifier](https://github.com/Simon-Martens/objekte/blob/cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/verify.py#L300-L369)

Use the same four input filenames in the test writer and reader. Run a valid control through the same temporary-directory CLI path first and require a successful comparison. For each mutation require a nonzero exit, its expected validation diagnostic and no newly written success file. For the duplicate case replace one row with another so cardinality stays at three and the duplicate-name check executes. Retain optimized-Python coverage.

## 2. Correct the trace extractor and test it against trace text

For the strace line `pread64(3</fixture/data/00.json>, "hij", 3, 7) = 3`, the current extractor reports offset **3**. The actual offset is **7**. It takes the requested length from the penultimate argument. It also silently drops a successful read when strace prints separate `<unfinished ...>` and `<... read resumed>` lines. I reproduced both with recorded-format input. The committed compact JSON retains neither the syscall name nor the original trace lines. [Trace extractor](https://github.com/Simon-Martens/objekte/blob/cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/trace_io.py#L58-L111)

Read the final `pread64` argument as the offset and the successful return value as the byte count. Handle split calls by thread or change capture to produce complete per-thread lines and merge them correctly. Reject ambiguous or unsupported in-scope fixture operations explicitly. Exercise a nonzero-offset partial positioned read, a split read, and a normal read following a seek. Check the exact path, offset, successful byte count and occurrence count. Retain a small filtered trace excerpt or equivalent raw input alongside the compact results so the extraction can be replayed.

## 3. Verify the newly generated evidence end to end

- The README currently writes new results under `$evidence_dir` and ends by verifying the checked-in directory. Give full commands for both revisions, with distinct raw log, marker and result paths. Point the final verifier at `"$evidence_dir"`. Use an absolute path for the verifier itself so either checkout works. [Reproduction commands](https://github.com/Simon-Martens/objekte/blob/cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/README.md#L24-L75)
- In `read_probe.rs`, assert that the single `exact_page` winner returns the expected logical object from `00.json`. The probe currently serializes IDs, counts and counters without checking the returned value. Keep this assertion outside the query timing interval and use fixture values for the expectation. [Read probe](https://github.com/Simon-Martens/objekte/blob/cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca/docs/ai/generated/core-refactoring-completion/assignment-2-source-preparation/read_probe.rs#L112-L149)
- Rerun the four-file, three-query comparison at A1 and the reviewed repair source with the corrected evidence tools. Check ordered IDs, expected payload, distinct paths, successful opens, read multiplicities and total bytes. Establish whether the reported totals of **60**, **330** and **286** bytes reproduce. Explain any difference using the raw trace. Keep the inherited public-counter defect in its existing issue.
- Append the corrected command, tested revisions and outcomes to the Assignment 2 report. Keep its Linux-only Rust results and known formatter conflict explicit. The full-corpus benchmark belongs to Assignment 3.

**Sign-off condition.** A clean reproduction validates its own generated files. Each negative control fails for its intended reason, trace extraction preserves the known syscall offsets and multiplicities, and the corrected comparison establishes A1 parity. These checks can be completed without changing the core query implementation.

## Implementation report

- Task identifier: `core-refactoring-completion-assignment-2-verification-addendum`
- Completion date: 2026-11-19
- Reviewed delivery: `cf6aa2b32f4ecd8c7390902553dc376d3f3e0cca`
- Tested A1 source: `b4172a1749750ee1c87ef74a56f5ee7cec6c40ee`
- Tested repaired source: `d07b700a7188efd6b45ec542f197b78a211cb88a`
- Integration branch and worktree: `integration/core-refactoring-completion` /
  `/home/simon/source/objekte-integration-core-refactoring-completion`

The verifier now uses the same four filenames for self-test generation and
child validation. A valid comparison runs first through the temporary CLI.
Every negative mutation then runs as an optimized child process and must exit
nonzero, emit its intended revision/query/field diagnostic and leave no output
file. Covered negatives are paired empty, missing, duplicate and unknown
queries; one-sided count; omitted counter; reordered IDs; and altered trace
offset.

The trace extractor now reads positioned-read offsets from the final
`pread64` argument and byte counts from successful return values. It tracks
remaining arguments, seek state and fd paths, and merges split calls by thread
and syscall. Unsupported in-scope operations fail explicitly. Its self-test
checks a positioned read at offset 7, split open/read calls and a normal read
following `SEEK_SET`; `trace-excerpt.log` retains the raw test format.

`read_probe.rs` now derives its tested SHA from the checkout, fails against an
unexpected `--expected-sha`, emits query timing markers and asserts the exact
logical `00.json` payload outside the timed interval. The README contains full
A1 and candidate commands, distinct raw log/marker/result paths and an absolute
tool path. Its final verifier consumes the newly generated evidence directory.

The corrected rerun at A1 and `d07b700a` reproduced:

| Query | Opens | Distinct paths | Read ranges | Bytes |
|---|---:|---:|---|---:|
| `exact_page` | 1 | 1 | `00@0+60` | 60 |
| `combined_old` | 6 | 3 | `00@0+60, 01@0+59, 03@0+46` twice | 330 |
| `combined_number` | 5 | 3 | `00@0+60, 01@0+59, 02@0+48, 00@0+60, 01@0+59` | 286 |

Ordered IDs, payload, opens, range multiplicities and byte totals are equal
across revisions. The inherited public distinct-file/byte counter defect
remains recorded in
[`query_source_counter_semantics.md`](../issues/bug/query_source_counter_semantics.md).

The original Assignment 2 Rust gates remain green on Linux; no Windows runner
was available. The established formatter conflict remains explicit: plain
rustfmt requests one blank line around `model/json.rs`'s two `impl` blocks,
while the repository spacing convention accepts two and the script still
reports its inherited 199-file baseline.

The detached A1 and candidate comparison worktrees and temporary evidence
directory were removed after compact results were retained. The integration
branch and worktree remain for owner review.
