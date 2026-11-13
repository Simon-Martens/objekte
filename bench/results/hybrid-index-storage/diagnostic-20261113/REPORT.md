# Hybrid index storage: integration diagnostic, 2026-11-13

Task `hybrid-index-storage` remains **incomplete**. The implementation is on
`integration/hybrid-index-storage`, with base/master
`da8fde68472b8646a63a05b3b3654c58de877301` and measured candidate
`389375b06d06e96229df60eb064b21dc81ac3fba`. Master was not changed.
The authoritative task specification remains untracked in the original
checkout's `docs/ai/tasks/` directory. The task's acceptance gates prohibit
marking it done while construction and query regressions remain.

## Implemented behavior

| Policy | Query representation | Persistence and readiness |
| --- | --- | --- |
| Memory only (default) | Owned, sealed segment readers | No managed index writes; complete build and updates seal before publication. |
| Temporary, repository, or custom; on demand | Checked mapped segment readers | A complete committed generation opens before source reconciliation; completed updates synchronize in coalesced background batches, with explicit flush. |
| Same disk locations; fully resident | Checked owned segment readers | Opening includes payload loading and required validation; updates seal new segments into owned buffers. |

The managed store uses immutable segment and catalog files, a generation
manifest, cross-process locking, atomic publication, pinned readers, and
ownership-checked cleanup. The CLI/TUI and desktop use the shared storage
policy and opener. Production consumers reject the deprecated independent
snapshot flags and write paths. The current core includes source-free
compaction, a 64 sparse-segment admission limit, and a catalog-history rebase
when unreferenced historical metadata exceeds twice the current segment file
references (with a 64-entry floor). The 70-update history test observed a
drop from 65 historical entries to 3 and a correct reopened query.

The complete commit list, including merged subtask commits, is in
`implementation-commits.txt`. Principal integration milestones are
`bfca5ba9d2` (contracts/baseline), `083fc1e6b0` (checked format),
`947d66e10b` (managed lifecycle), `56b178cdad` (streamed writes),
`679c83b720` (sealed reopen), `352600791a` (CLI/TUI), `796667044c`
(desktop), `8b166cc746` (memory sealing), `d0938dfbe5` (fresh managed
sealed readers), `33d1af0a8e` (bounded hot posting cache), `5f44939513`
(catalog-history rebase), and `389375b06d` (explicit flush benchmark).

## Correctness evidence

The established extended Musenalm campaign contains 220 cases, tested in
ASCII and Unicode with five measured repetitions: 440 case/profile pairs and
2,200 query samples per mode. Memory, on-demand, and fully resident modes
each had zero result-digest differences from the archived baseline, zero
non-`ok` samples, and zero verified-result-cache hits. The 30-repetition
five-case tail campaign likewise had matching digests and zero result-cache
hits in all four modes. Corpus fingerprint:
`ad517d15e3216bfbdb569906d14a1717271daaf27a8d58e47e55e1e63e5db868`.
Full core tests after the history fix: 624 passed, 16 ignored, plus all core
integration tests. The full workspace tests passed before that final core
fix; the core suite and release benchmark builds passed afterward. Native
desktop open/reopen and CLI/TUI disk/memory smoke tests passed earlier in
integration. These are meaningful correctness checks, but they do not prove
every failure and residency scenario listed in the specification.

## Matched performance measurements

Release binaries were rebuilt from the pushed candidate commit. The retained
baseline binaries come from `da8fde6847`. See `paired-builds/trials.json`,
`paired-opens/trials.json`, and `paired-flushes/trials.json` for exact binary
SHA-256 hashes, commands, order, and every sample. Each comparison has seven
fresh-process trials with rotating execution order. The same 148,486-record
configured Musenalm fixture, ASCII alphabet, and 32 build workers were used.
The baseline snapshot is 132,710,084 bytes. The Btrfs/NVMe environment,
compiler, CPU, and fixture are described in the parent archive README.
Host-wide page caches were not cleared; opening results are warm-cache
results. A separate controlled cold-index result is still required.

| Measure (seven-trial median unless noted) | Baseline | Candidate | Gate/result |
| --- | ---: | ---: | --- |
| Memory construction | 1,002.38 ms | 1,710.94 ms | 1.707×; **fails** <=1.05× |
| On-demand stored opening | 652.35 ms | 386.53 ms | 40.7% faster; opening-only target passes |
| Fully resident stored readiness | 652.35 ms | 487.63 ms | 25.2% faster; readiness target passes |
| First complete flush/save | 388.07 ms | 754.30 ms | 1.944× slower; material save tradeoff |
| Retained RSS after construction | 537.8 MiB | 650.4 MiB | +20.9%; **fails** 10% budget |
| Peak RSS during construction | 547.0 MiB | 706.4 MiB | +29.1%; **fails** 10% budget |
| Retained RSS after fully resident open | 312.4 MiB | 317.0 MiB | +1.5%; within budget |
| Retained RSS after on-demand open | 312.4 MiB | 264.8 MiB | 15.2% lower |
| Stored bytes | 132.71 MB | 128.31 MB | 3.3% lower |

The baseline and candidate build intervals include all work before a complete
searchable index is returned. A temporary per-segment diagnostic in
`seal-profile.log` attributed 469.8 ms to owned segment writing and 173.7 ms
to decoding/validation across eight shards (643.6 ms total). Its
instrumentation was removed before the final matched runs, so this is an
attribution estimate rather than a second acceptance sample. The largest
131,900-record shard alone accounted for 270.9 ms writing and 154.2 ms
decoding. The final unprofiled trials do not separate every admission and
encoding component requested by the specification.

The seven opening trials measured preparation only. Candidate reopen samples
confirmed zero source reads/build parses. Opening plus the first complete
query was not captured as one matched timer for the baseline, so the **full
opening gate remains unproven**, despite the opening-only improvements.

## Query latency and tails

The primary result cache was disabled. The archived baseline and exact-head
candidate each ran five repetitions of all 440 ASCII/Unicode case/profile
pairs. Geometric mean of candidate/baseline per-case medians:

| Mode | Latency ratio | Gate |
| --- | ---: | --- |
| Memory only | 1.082 | **Fails** <=1.05 |
| Fully resident stored | 1.143 | **Fails** <=1.05 |
| On-demand stored, warm workload | 1.177 | **Fails** <=1.05 |

Among 30 repetitions of each representative ASCII case, Q175
(`entry.year >= 1800 SORT BY numbering ASC`) was reproducibly slow:

| Q175 | Baseline | Memory | Fully resident | On demand |
| --- | ---: | ---: | ---: | ---: |
| Median | 4.634 ms | 6.525 ms | 6.932 ms | 6.993 ms |
| p95 | 4.785 ms | 8.782 ms | 9.011 ms | 9.345 ms |

Q167 and Q177 also exceeded the 1.10 p95 target in the final 30-repetition
set; Q001 and Q202 improved. All raw per-query samples, counters, plans,
digests, and commands are under `queries/` and `tail-30/`. These campaigns
do not establish constrained-memory cold-access performance or an independent
inference-enabled performance result. The bounded decoded-posting cache is
included in the candidate query timings; its allowed capacity is 5% of
encoded payload per segment, capped at 8 MiB per segment.

## Remaining acceptance work

The construction, build-memory, aggregate query, and representative tail
gates fail. Full opening-plus-first-query, controlled cold-index access,
inference-enabled and ordinary/small-fixture performance, and sustained
update/compaction costs have not been established. Compaction currently
responds to the segment-count limit but lacks the requested independent
size and retired-ratio triggers and bounded incremental maintenance proof.
Managed opening still classifies some store access failures as invalid-cache
errors, although it returns the error rather than silently rebuilding.
These limits should be addressed and retested before moving the task to
`docs/ai/done/` or asking the user to merge the integration branch.
