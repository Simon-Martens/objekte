# Hybrid index storage construction and query regressions

- Category: `perf`
- Originating task: `hybrid-index-storage`
- Branch/revision: `integration/hybrid-index-storage` at `389375b06d06e96229df60eb064b21dc81ac3fba`, compared with master baseline `da8fde68472b8646a63a05b3b3654c58de877301`.
- Code areas: `crates/core/src/hybrid_segment_codec.rs`, `posting_dictionary.rs`, `posting_values.rs`, `run.rs`, and `lifecycle_open.rs`.

## Observation and impact

Seven paired fresh-process ASCII builds on the 148,486-record Musenalm fixture
had median construction times of 1,002.38 ms baseline and 1,710.94 ms
candidate. Candidate after-build RSS was 650.4 MiB versus 537.8 MiB, and peak
RSS was 706.4 MiB versus 547.0 MiB. The task's <=1.05 construction ratio
and <=10% build-memory increase limits fail. A temporary sealing profile
attributed 643.6 ms across eight shards to segment writing and decoding;
the 131,900-record shard accounted for 425.2 ms of that total.

The full 220-case ASCII/Unicode campaign, five repetitions and result cache
disabled, found geometric mean latency ratios of 1.082 for memory, 1.143 for
fully resident, and 1.177 for warm on-demand storage. All results matched
baseline digests. For Q175, 30-repetition p95 rose from 4.785 ms to
8.782/9.011/9.345 ms in those modes. The <=1.05 aggregate and <=1.10
representative p95 limits fail. First full managed flush was also 754.30 ms
versus 388.07 ms for baseline snapshot save, although stored bytes fell by
3.3% and stored opening improved.

## Reproduction and evidence

Build the release examples at the candidate revision, then run the archived
scripts and query commands in
`bench/results/hybrid-index-storage/diagnostic-20261113/` against the
configured `examples/musenalm` fixture. That directory contains the exact
binary hashes, command lines, raw paired samples, query counters, result
digests, and the full [diagnostic report](../../../../bench/results/hybrid-index-storage/diagnostic-20261113/REPORT.md).

## Why unresolved and possible approaches

The current implementation must seal memory-only segments before declaring
construction complete. Moving sealing beyond the timer would violate the
task's readiness contract. The repeated query regressions remain after a
bounded hot-posting cache was added; caching every tiny posting worsened the
measured geometric mean and was reverted. These are measured findings.

Possible next approaches, not yet established by measurement, are a direct
in-memory sealed builder that avoids a write/decode round trip, selective
layout improvements for the broad relation-and-sort queries, or a narrower
bitmap decode path that preserves validation and exact result ordering.
Each change needs a fresh matched campaign. The integration branch remains
unmerged and the task document remains in `tasks/` until the gates pass or
the user explicitly changes the acceptance contract.

## 13 November 2026 repair checkpoint

The user subsequently directed that small timing differences be improved,
documented against baseline, and not treated as absolute completion limits.
The full [repair checkpoint](../../../../bench/results/hybrid-index-storage/repair-20261113/final-e15b/REPORT.md)
uses seven paired build/open/flush trials and all 220 cases in ASCII and
Unicode with 30 repetitions and a common raw-sample statistics definition.
At `e15b3f5c4b`, construction fell from 1.707 to 1.130 times baseline;
retained and peak build RSS were 1.056 and 1.071 times baseline. Fully
resident and on-demand opening took 0.696 and 0.606 times baseline, while
first flush took 0.673 times baseline. Query median geometric means were
1.075 memory, 1.091 resident, and 1.092 on-demand; every result digest
matched. Broad relation/sort and source-heavy queries still show measurable
latency overhead. The older measurements above remain valid for their named
revision and are not overwritten.
