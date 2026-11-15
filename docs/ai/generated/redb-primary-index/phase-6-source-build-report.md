# Phase 6 direct source construction report

- Task: `redb-primary-index`
- Branch: `integration/redb-primary-index`
- Date: 2026-11-15
- Status: **direct source-to-redb construction works; mandatory construction/memory gates fail**

## Implementation

`index_store/store_build.rs` now:

- discovers selected files directly into the redb catalog (no native `FileInventory`);
- schedules the existing parallel extraction jobs (`process_repository_file_batches`);
- admits each `BatchContribution` in one transaction: global slot allocation,
  record/logical-ID/file rows, value and length blocks, compact state masks,
  sort entries, relation occurrences, text FTS blocks and one-copy
  `record_grams`;
- does not construct or retain a native `Repository`.

Command:

```sh
cargo build --release --locked -p objekte-search-core \
  --example bench_index_store --features store-feasibility,store-reverse-lz4
target/release/examples/bench_index_store --build-source \
  --root /home/simon/source/objekte/examples/musenalm --alphabet ascii
```

## Result (ASCII, 148,486 records)

| Metric | Result | Gate |
| --- | ---: | ---: |
| Complete construction | 20.29 s | ≤ 1.15 × 0.99 s ≈ 1.14 s |
| Post-build RSS | 829.9 MiB | A3 limit 638.5 MiB |
| Peak RSS | 839.4 MiB | A3 limit 745.0 MiB |
| Table pages | 510.6 MiB | diagnostic |
| Records/files | 148,486 / 148,486 | expected |

The build is functionally complete and writes every selected record, but it
misses both mandatory gates. The construction regression is dominated by the
application-level membership/sort/gram writes and redb copy-on-write page
churn, not source parsing: the earlier fixture import of the same facts took
8.76 s, and native construction takes ~1 s on this host.

## Required decision

Phase 6 cannot pass the unchanged construction/peak-memory gates with the
current representation. Options:

1. Optimize the admission path further (batch table handles/allocator) before
   another measurement. The fixture-import floor of 8.76 s makes the 1.15×
   construction gate unlikely without a representation change.
2. Amend the construction-time and/or peak-memory gates explicitly.
3. Change the structured/sort/gram representation again.

No Phase 7 consumer migration was started. Evidence:
`bench/results/redb-primary-index/feasibility-a3/build-source/ascii.json`.
