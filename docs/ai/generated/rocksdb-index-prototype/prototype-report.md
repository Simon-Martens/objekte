# RocksDB index prototype report

Date: 2026-11-15  
Task: `rocksdb-index-prototype`  
Base: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Integration branch: `integration/rocksdb-index-prototype`

## Decision

**Reject the RocksDB construction design for production migration.** The
measured basic ASCII build is 81.56 times the matched pristine-native build and
uses 12.49 times its peak RSS. This exceeds both the 1.15 time gate and the
1.60 peak-memory gate by an order of magnitude. The specification's early-exit
rule therefore applies. Later primitive-query, mutation, Unicode-corpus and
production-integration work was intentionally not undertaken.

This is a measured rejection of this design, not a claim that RocksDB cannot be
made faster. It answers the assigned prototype question without launching a
broad tuning campaign.

## Implemented evidence

- One default column family, pessimistic `TransactionDB<SingleThreaded>`,
  shared gate, per-file receipts and ordered transaction mutation plans.
- Direct source extraction into flat typed facts without constructing a native
  `Repository` or a retained native index for the candidate.
- Whole-physical-file atomic admission across all selected collection
  memberships.
- Concurrent preparation and admission workers. Preparation and write workers
  use separate thread lifetimes and bounded channels.
- Exact finalization from committed records and blocks into value/length
  directories, structural and gram statistics, field coverage and collection
  counters. No provisional maximum or batch-local cardinality is published as
  final.
- Memory-mode host-directory isolation and file-mode primitives retained from
  Phase 1.
- Canonical native/prototype family digests for files, records, values,
  lengths, states, domains, grams, sorts and relations.
- Full-corpus internal reverse-membership and cardinality validation.

## Measured campaign

The commands were built once in release mode and invoked as fresh processes.
Both baseline and candidate use the basic ASCII Musenalm definitions,
one-record-per-file input, P=8 preparation workers and the bundled RocksDB
library.

| Configuration | Readiness/build time | Resident RSS | Peak RSS | Admission transactions |
| --- | ---: | ---: | ---: | ---: |
| Pristine native | 0.936 s | 394,117,120 B | 404,692,992 B | n/a |
| Candidate native control | 0.967 s | 401,756,160 B | 412,430,336 B | n/a |
| RocksDB W=1 | 155.064 s | 4,630,896,640 B | 4,901,682,496 B | 148,486 (pre-accounting run) |
| RocksDB W=2 | 92.617 s | 4,730,003,456 B | 5,190,975,488 B | 148,486 (pre-accounting run) |
| RocksDB W=4, final | 76.344 s | 4,766,621,696 B | 5,054,058,496 B | 582 |
| RocksDB W=8 | 72.816 s | 5,046,358,016 B | 5,534,969,856 B | 148,486 (pre-accounting run) |

The final W=4 run produced 148,486 records and physical files. Its transaction
count is 582 because 581 bounded preparation chunks plus the isolated first
file are admitted in whole-file transaction batches.

The candidate native control has a 1.033 time ratio, 1.019 resident-memory
ratio and 1.019 peak-memory ratio versus pristine master. This satisfies the
native-control comparability check. An earlier candidate control was 4.26 s
because the copied scheduler incorrectly multiplied normal preparation weights
by eight even when the RocksDB feature was not constructing a store. Restoring
the master scheduler weights removed that regression; the fixed control above
is the one used for comparison.

## Dominant cost and repair

The first full W=1 construction took 345.661 s and committed one transaction per
physical file. A localized repair grouped prepared files into bounded,
whole-file admission transactions. This reduced W=1 readiness to 155.064 s.
Writer scaling then reduced readiness to 92.617 s at W=2 and 76.344 s at W=4.
W=8 was within 4.7% of W=4 while increasing memory, so W=4 is the selected
configuration.

The final W=4 stage accounting is:

- Discovery: 3.854 s.
- Summed preparation-worker work: 8.843 s.
- Coordinator mapping: 0.552 s.
- Writer-queue backpressure: 53.174 s.
- Summed admission-worker work: 216.800 s.
- Admission wall interval: 54.510 s.
- Finalization: 17.845 s.
- Total readiness: 76.344 s.

Admission remains the dominant interval. Four writer threads can process
independent physical-file work, but common structural and gram blocks still
serialize many transactions. The experiment does not justify reallocating
slots, enlarging buffers, or adding a second engine to hide this result.

## Correctness

The final full-corpus validation completed with zero errors:

- 7 collections and 148,486 records.
- 148,486 file records and build receipts.
- 2,814,411 value memberships.
- 921,173 length memberships.
- 14,004,908 structural state memberships.
- 56,275,908 gram memberships.
- 566,768 sort entries.
- 152,544 value-directory rows, 641 length-directory rows and 418 coverage rows.

Independent native/prototype exact-fact comparisons pass on fixtures for
overlapping multi-record collections, typed scalar and numeric values, arrays,
structural states, domains, normal/skip grams, sort keys, empty files and a
malformed JSONL fallback. Relation comparisons remain covered by the canonical
digest implementation but were not exercised by these fixtures or the basic
measured corpus.

The memory-mode host directory remained empty. Phase 1 tests continue to cover
snapshot retention, same-block concurrent updates, empty memory hosts, file
mode flush/reopen and malformed codecs.

## Requirements not completed under early exit

- Representative primitive-query timings against native equivalents.
- The retained-reader source-free mutation campaign.
- File-backed measured-corpus durable reopen.
- Unicode measured-corpus construction and three alternating measured pairs.
- Full-corpus independent native reference export; the full-corpus run used
  the internal invariant validator, while exact native comparison ran on
  fixtures.
- Query planner, CLI, desktop, inference and production storage integration.

These omissions are consequences of the measured rejection and must not be
reported as passing.

## Reuse and adaptations

Phase 2 reused storage-independent donor extraction and fact shapes, with
prototype-native RocksDB ownership, scheduling, transactions, receipts,
finalization and validation. The detailed source list and changed semantics are
recorded in `reuse-manifest.md`.

The implementation deliberately does not use redb transactions, table handles,
page accounting, donor benchmark control flow, optimistic transactions, Merge
operators, SST ingestion, per-worker databases, larger caches/buffers, alternate
block sizes or additional compressors.
