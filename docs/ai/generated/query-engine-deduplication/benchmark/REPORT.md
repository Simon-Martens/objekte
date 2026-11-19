# Query engine deduplication benchmark

## Setup

- Baseline revision: `d35ddde56e17114da35d1a76e776615262897409`
- Benchmarked candidate revision: `093726e6b4b8168499133bd35086188e50e93b64`
- Production implementation tip: `4fb6ffdc9821c336ff205c66064ebf42b45d17a9`; `093726e6b4` adds test-only timing probes.
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Machine: 32-thread 13th Gen Intel Core i9-13900K
- Profile: release, ASCII bigrams, four query workers, zero verified-cache bytes
- Corpus: `/home/simon/source/objekte/examples/musenalm`
- Corpus fingerprint on both sides: `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`
- Baseline executable SHA-256: `871abbbfa8dcea7cece6618f29d276f6b2aa0118793c4dfb1bd1582bb73868ac`
- Candidate executable SHA-256: `3eab475a5739e7c39c8311ebbdc6c0dc9f6b6721d2ab687c7db83e960824ad3e`

The baseline and candidate example binaries were built separately. The same
queries, corpus, alphabet, workers, cache setting, and repetition count were
used. `queries.json` and `schema-manifest.json` are byte-identical between
`baseline` and `final-candidate`.

## Musenalm

Command, run once per revision:

```text
bench_musenalm --root /home/simon/source/objekte/examples/musenalm \
  --cases Q001,Q042,Q083,Q136 --profiles ascii-off --repetitions 3 \
  --query-workers 4 --verified-cache-bytes 0 --result-dir <result-dir>
```

| Case | Baseline median | Final candidate median | Change |
| --- | ---: | ---: | ---: |
| Q001 | 18.643 ms | 13.334 ms | -28.5% |
| Q042 | 95.207 us | 102.898 us | +8.1% |
| Q083 | 6.865 ms | 6.734 ms | -1.9% |
| Q136 | 3.982 ms | 3.829 ms | -3.9% |

Every repetition passed on both revisions. Ordered-result digests are
identical, as are all work columns after removing timing-only CSV fields:
matches, returned records, candidates, files, bytes, parses, exact-posting
matches, scanned records, residual evaluations, cache hits, and planning work.

Q042 was checked with three alternating fresh baseline/candidate batches:

| Batch | Baseline median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| 1 | 106.745 us | 100.427 us | -5.9% |
| 2 | 106.586 us | 103.344 us | -3.0% |
| 3 | 109.811 us | 110.975 us | +1.1% |

The alternating mean of medians changed from 107.714 us to 104.915 us
(-2.6%). Ordered digests and work counters are identical in all pairs. After
removing the independently measured `diagnostic_ns`, the plan JSON is
identical for every pair: exact `profession = "Text"` posting, 1,520
candidates, stored-order page, and no source reads.

The run manifests have different workload fingerprints because that benchmark
fingerprint includes the workload file's checkout path. Both workload files
have Git blob `39dcfe72d4e735cd4da392381517a62bcc493a8b`. The materialized
queries and schema manifests are byte-identical.

## Deterministic relation fixtures

The existing `both_physical_strategies_keep_independent_witnesses_and_reuse_targets`
and `related_sort_does_not_reuse_an_incomplete_filter_target_projection`
tests were run in release mode with test-only timing probes enabled. Each timed
one warm, already-built search repeated 21 times. Fixture construction,
reference search, and explain calls were outside each timed interval.

| Case | Baseline median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| `indexed_relation_filter` | 18.846 us | 16.797 us | -10.9% |
| `selected_relation_fts` | 15.025 us | 15.057 us | +0.2% |
| `source_dependent_residual_sort` | 35.423 us | 33.857 us | -4.4% |

The target-first case retains one target read and 40 reverse members. The
source-restricted selected-FTS case retains its route. The residual sort keeps
two sort-target reads. These are the specialized behaviors required by the
specification.

## Validation

- `cargo test --locked -p objekte-search-core`: passed, 617 unit tests and all
  integration/doc tests; 16 manual/large tests ignored as designed.
- `cargo test --locked -p objekte-search-core --test relation_oracle --test relation_occurrence_oracle --test non_json_relations`: passed.
- `cargo test --locked -p objekte-search-core` filters for
  `query_execution::relations::evaluation`, `query_access::relations::semijoin`,
  `query_execution::restrictions`, `query_execution::runner`, and
  `query_execution::streaming`: passed after each relevant implementation
  commit.
- `cargo check --locked --workspace --all-targets --all-features`: passed.
- `cargo test --locked --workspace`: passed.

Raw manifests, plans, measurements, query result rows, and ordered digests are
retained in `baseline/`, `final-candidate/`, and `alternating/`.
