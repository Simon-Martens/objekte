# Staged index construction benchmark evidence

The implementation and analysis are in
[`docs/ai/generated/staged_index_construction_results.md`](../../../docs/ai/generated/staged_index_construction_results.md).
The completed predecessor is revision `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`;
the combined staged implementation measured by the full suite is
`21265636a1b799d8376d700fc74eb67f787c79e6`. Each run manifest records
its exact command, settings, corpus fingerprint, and source revision.

| Directory | Evidence |
| --- | --- |
| `baseline-suite/` | Predecessor, 194 cases, four profiles, three repetitions |
| `final-{base,partial,complete,inference}/` | Final staged matrix, 202 cases, four profiles, three repetitions, plus correctness comparisons |
| `readiness-final/` | Seven paired warm and advisory-evicted predecessor/staged trials |
| `readiness-prefixed/`, `readiness-postfix-probe/` | Before and immediately after the catalogue callback fix |
| `focused-{base,foreground}/` | Seven-repeat focused query timings with optional work paused or active |
| `shapes-final/` | Seven-repeat synthetic sparse/mixed/dynamic-field fixture |
| `cancellation-final/` | Seven one-query process trials measuring `Index::quiesce()` while pass 2 was running |

The large binary snapshots stay in the ignored `target/` benchmark directories.
Their byte sizes and save/restore measurements are in `snapshot-results.json`
and the report. `query-plans.jsonl` files that were empty were omitted.
The advisory eviction logs and build/test logs also stay in `target/`; the
numeric summaries, commands, manifests, counters, result digests, and samples
needed to audit the report are retained here.
