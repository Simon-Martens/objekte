# Mixed-domain selective posting comparison

The fixture generator writes 10,000 healthy nonmatches (one deliberately
contains `needle` in an unrelated field), one indexed `title = needle` match,
and one malformed JSONL fallback record. Its SHA-256 is
`f61aef8724a3393d40a902137ded32c399999b1fcdd9199abe8c8c64d27be229`.
The healthy control omits only the malformed final line and has SHA-256
`83a2512b6071e4b361bafdf5c87cc48c65479a0fd2476467ad667e0ddd70c40f`.
The benchmark example is identical in both worktrees; A uses the unmodified
`92cdb92b19` core, B uses the repair worktree. Each release binary opened the
same physical fixture with verified cache disabled, 32 workers, locator
output, count and evidence enabled. Query warmup preceded 11 timed requests
per process block. Blocks ran A-B-B-A; all raw samples and counters are saved.

| Fixture | A median ms (min–max) | B median ms (min–max) | Difference ms |
|---|---:|---:|---:|
| Mixed | 2.311 (2.242–2.663) | 0.106 (0.061–0.117) | -2.205 |
| Healthy control | 0.099 (0.058–0.107) | 0.104 (0.063–0.112) | +0.005 |

For the mixed query, A planned all 10,002 slots, opened the JSONL source once,
read 179,978 bytes and parsed 10,001 logical records. B planned one slot,
opened no source, read no bytes and parsed no records; the one result was
proven by the posting. The healthy control remained index-only in both
variants, with zero opens and parses. The large mixed-case change is
structurally attributable to removing the collection-wide fallback. The
5 µs healthy-control difference is too small to interpret from these samples.

The source fixture and query definitions are fixed in
[`generate_mixed_planner_fixture.py`](../../../generate_mixed_planner_fixture.py)
and [`bench_mixed_planner.rs`](../../../../crates/core/examples/bench_mixed_planner.rs).
