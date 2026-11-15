# RocksDB construction repair report

Date: 2026-11-15
Task: `rocksdb-construction-repair`
Parent prototype: `63b212fcf9a6cb6fb42af8fdee8a29cd2fce7e3b`
Native base: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`
Integration branch: `integration/rocksdb-construction-repair`

## Outcome

**Performance failure.** The repair materially improves the prototype but does
not pass the unchanged construction screen.

| Observation | Pristine native | Repaired RocksDB W=4 |
| --- | ---: | ---: |
| Caller construction time | 1.079 s | 35.650 s |
| Resident RSS | 395,444,224 B | 1,093,632,000 B |
| Peak RSS | 404,967,424 B | 2,432,851,968 B |
| Time ratio | 1.00 | 33.03 |
| Resident ratio | 1.00 | 2.77 |
| Peak ratio | 1.00 | 6.01 |

The time, resident-memory and peak-memory gates all fail. The candidate-native
control is 0.9609 s, or 0.89 times the pristine control, so the result is not
caused by a native-path regression.

The repair reduced readiness from the parent prototype's 76.344 s to 35.650 s
and peak RSS from 5.054 GB to 2.433 GB. Production migration therefore remains
stopped.

## Answers to the repair questions

1. **Coverage rows and receipts.** The parent prototype wrote one receipt per
   physical file and repeated per-file coverage rows. The repaired store writes
   582 receipts and 25,586 pre-cleanup coverage deltas. Finalization deletes all
   25,586 and verifies that zero remain. The exact historical coverage-row
   count was not instrumented; the specification's approximately 6.4 million
   figure was source-derived.
2. **Metadata limits.** Every ordinary discovery and finalization batch obeyed
   both limits. Discovery used 23 batches, with the largest at 2,693,283 bytes
   and 32,765 operations. Finalization metadata used 7 batches, with the
   largest at 4,009,510 bytes and 32,768 operations. The raw JSON has the exact
   values.
3. **Charged work.** Peak measured ready prepared data was 25,082,220 bytes,
   below the 64 MiB target. The initial 16-job scheduling estimate was
   236,667,936 bytes and exceeded the target before actual capacities replaced
   it. No actual retained-capacity overshoot was observed. The in-flight count
   remains capped at `min(16, 2P+W)`.
4. **Admission costs.** Across four workers, static planning totals 20.038 s,
   database work 107.006 s, and commit 2.124 s. Summed worker admission is
   129.843 s; admission wall time is 32.789 s. The plan contains 2,692,703
   block reads, 5,219,688 keys and 854,242,205 encoded bytes. Planning now
   completes before the transaction opens. Get-for-update time is not labeled
   lock wait.
5. **Contention.** Direct lock-wait time is unavailable from the supported
   benchmark metrics. No retries occurred in the final 582-attempt campaign.
   Contention is therefore not established as the dominant cause.
6. **Memory attribution.** Application-owned queue measurements omit native
   allocator and RocksDB interior allocations. The repair removes per-file
   descriptor/coverage duplication, but the remaining 2.433 GB HWM includes
   prepared facts, five million mutation operations, memtables, block cache,
   virtual files and allocator retention. Exact partition is unavailable.
7. **Provenance.** The final run uses the clean `0147a0cb73` binary with
   SHA-256 `a7da2b6e636b7b5a1fe2ca361a375060b696af841a7f1c5a6ea9ff886dfec66f`.
   `comparison.json` rejects mismatched schema/configuration manifests,
   missing validation and validation errors, and records SHA-256 digests for
   every raw input.
8. **Correctness.** Full ASCII internal validation passes with zero errors:
   148,486 records/files, 582 receipts accounting for every file,
   2,814,411 values, 921,173 lengths, 14,004,908 state memberships,
   56,275,908 gram memberships and 566,768 sort entries. Version-1 format
   rejection, bounded metadata deletion, overlapping atomic admission and
   empty/malformed files are covered by focused tests. Native exact-fact
   comparison remains fixture-scoped; no positive recommendation follows.
9. **Decision.** Performance failure under unchanged gates. Do not proceed to
   production integration or another design/tuning cycle.
10. **Still unimplemented.** Query planner/DSL/streaming, CLI/desktop,
    inference, slot reuse, concurrent lifecycle mutation, crash resume,
    memory-to-disk export, full file-backed corpus reopen and Unicode corpus
    work remain deferred.

## Historical baseline discrepancy

The completed parent report embedded a 936,001,020 ns native sample, while its
committed `baseline/ascii-p8/build.json` contains a different sample. The new
repair does not rewrite either historical value. The available artifacts do
not contain enough run identity to reconstruct why two native samples were
mixed. This repair uses only a fresh pristine P=8 control
(`1,079,312,756 ns`, `395,444,224 B` RSS, `404,967,424 B` HWM) and a fresh
candidate-native control from the same corpus/schema configuration.

## Repair changes

- Separate explicit admission-batch IDs from physical file IDs.
- One receipt and one compact coverage delta per admission transaction.
- Ordered file/version identities and content-derived replay digests.
- Prototype format version 2; version 1 is rejected.
- Shared 8 MiB / 32,768-operation metadata writer for puts, deletes and atomic
  file groups.
- Bounded discovery registration and streamed coverage-delta cleanup with a
  zero-row postcondition.
- Static admission planning and contribution compression outside transactions.
- Reusable contribution scratch and exactly four total admission attempts.
- Separate planning, database, commit, queue, preparation and finalization
  timings.

No cache, write-buffer, block-size, compression, transaction engine, bitmap
layout or native production behavior was changed.
