# Query engine deduplication repair evidence

## Setup

- Branch point: `d06444547660693cf168bce45a9db8ee360c31cd`
- Candidate code: `da7a1e57faf29f0889592e5b3df56b2a9c4ffcf2`
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Machine: 32-thread 13th Gen Intel Core i9-13900K
- Profile: release
- Each batch: one warm search followed by 21 timed searches on an already-built fixture

The raw samples and work counters are in `benchmark.json`.

## Broad filter with no effective relation restriction

The query was:

```text
@file.status = 1 || entry.title = 'Hymn'
```

The OR prevents one-hop relation target pruning while the bound relation still
enters the merged relation runner. Explanation selected neither
`RelationTargetFirst` nor `RelationSourceRestricted`.

Both revisions returned IDs `m1` and the malformed-fallback identity, with
count 2, 8 candidates, 0 exact-posting matches, 10 file opens, 527 bytes read,
9 parses, 2 target reads and 3 edge visits.

Alternating batch-median changes were +5.1%, +10.2% and -2.3%, with a +4.0%
mean. The sub-100-us samples overlap substantially and each side contains a
large scheduler outlier. There is no repeatable route or work change; the
ownership repair removes a deep bitmap copy.

## Exact segmented page with evidence and verified cache disabled

The query selected 100 exact `kind = 'x'` slots and returned the first five
IDs on both revisions. Both report 100 candidates, 100 exact-posting matches,
zero source opens, zero bytes read and zero parses.

Three alternating batch medians were:

| Batch | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| 1 | 15.488 us | 11.314 us | -27.0% |
| 2 | 14.196 us | 11.308 us | -20.3% |
| 3 | 28.961 us | 11.830 us | -59.2% |

The candidate is consistently faster. The focused test also proves that a
losing slot with cache collection disabled invokes the evidence producer five
times for five winners rather than 100 times, and that a warm second page
reuses the complete cache with zero new evidence calls.

## Validation

Focused runner, relation-evaluation and semijoin suites passed. The final
workspace check and core test results are recorded in the completed task
specification.
