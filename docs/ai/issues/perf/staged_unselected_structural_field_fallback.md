# Unselected structural fields lose implicit acceleration

Category: `perf`. Originating task: `staged-index-construction`. Relevant
revision: `21265636a1b7` on `integration/staged-index-construction`; selection
is resolved in `crates/core/src/build.rs` and source fallback in
`crates/core/src/run.rs`.

The task's explicit field-selection policy builds compact states and optional
postings only for configured indexed fields when inference is off. Existing
Musenalm schema paths such as `data.gnd.preferredName`, `contributors`, and
`incipit_statement` are `indexed: false`. The predecessor's implicit
presence, length, and empty-string structures nevertheless accelerated their
queries. The configured staged complete build does not retain those implicit
structures, so these queries scan source even after selected work finishes.

| ASCII, cache off | Predecessor median | Configured staged complete | Staged explicit inference |
| --- | ---: | ---: | ---: |
| Q108 presence | 0.233 ms, 0 parses | 46.025 ms, 4,010 parses | source-free |
| Q116 contributors length | 10.580 ms, 0 parses | 369.833 ms, 133,948 parses | source-free |
| Q120 empty string | 8.440 ms, 0 parses | 342.533 ms, 133,948 parses | source-free |

All compared result IDs, counts, evidence, and ordering matched. Reproduce
with the predecessor `bench_musenalm --extended` and staged
`bench_staged_musenalm --mode complete`, then inspect Q108/Q116/Q120 in their
`query-results.csv`. Raw three-repeat results and comparison flags are under
`bench/results/staged-index-construction/`.

The expected behavior under the assigned specification is correct source
evaluation for unselected fields. The impact is a large latency regression
for callers who relied on the predecessor's implicit structural indexes.
Within this task, rebuilding those indexes automatically would violate the
explicit selection rule. Enabling explicit inference restores equivalent
coverage for these cases. Another possible approach is to mark these fields
as indexed in the repository configuration; that requires a deliberate
configuration choice and may increase build/storage work. A future policy
change to admit implicit structural indexes would need user review. This
report records the tradeoff; it does not authorize that change.
