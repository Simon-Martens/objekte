# A4 redb construction repair and architecture assessment

Date: 2026-11-15  
Task: `redb-primary-index`  
Integration branch: `integration/redb-primary-index`  
A4 implementation commit: `a61bbe1b8c`  
Starting revision: `d9d76c997a28e03662c99b361d37f84cfb705896`  
Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`

## Outcome

The authorized A4 correctness defects and transaction-access repairs were
implemented and validated. The final construction candidate is not acceptable:
it remains approximately 26.7 times slower than the native baseline and its
post-build RSS and process HWM exceed the unchanged A3 limits by about 618 MB
and 518 MB respectively.

Phase 6 remains the current phase. Phase 7 was not started. No performance or
memory gate was relaxed.

## Unit correction

A3's illustrative values 669.5 MiB and 781.1 MiB were decimal MB labels. The
byte formulas are authoritative:

| Baseline | Bytes | MiB |
| --- | ---: | ---: |
| Native post-build RSS | 478,220,288 | 456.066 |
| Native peak RSS | 488,243,200 | 465.625 |
| A3 post-build limit | 669,508,403 | 638.493 |
| A3 peak limit | 781,189,120 | 745.000 |

The A2 combined candidate value 661,000,192 bytes is 630.375 MiB, not the
number shown in the earlier illustrative label.

## Repairs and commits

- `1ca6ac7707` corrected source construction invariants and benchmark
  boundaries. It removed the duplicate live-record increment; validates file
  epoch and catalog version before allocation; preserves fingerprints,
  diagnostics and statuses; remaps record references; merges field coverage;
  admits inference and inferred reverse facts; makes source-build `--validate`
  functional; and times through final commit and buffer release.
- `b6b9ae2d1c` added detailed construction attribution. The pre-repair control,
  run with mixed construction and validation, was stopped after 9:56.83 with a
  2,446,737,408-byte HWM. That failed sample is retained under
  `bench/results/redb-primary-index/construction-a4/phase6-corrected/`.
- `ae65e29f2b` batched transaction table access. It introduced grouped
  directory/statistics updates, one-open batched record/file/contribution
  writes, bulk allocation and incarnation updates, shared FTS text
  accumulation, borrowed contribution serialization and path/no-path metadata
  separation. A diagnostic run completed in 24.33 seconds.
- `a61bbe1b8c` replaced the coordinator transpose with worker-side direct
  extraction into flat slot-sorted facts and a single bounded redb admission.
  It removed the superseded native batch conversion from the production redb
  path and added backend-length accounting.

## Correctness

The final ASCII Musenalm store was validated in a separate process after
construction. It has zero invariant errors and exactly matches the independent
native input manifest:

| Fact | Native manifest | redb final |
| --- | ---: | ---: |
| Collections | 7 | 7 |
| Files | 148,486 | 148,486 |
| Live records | 148,486 | 148,486 |
| Value memberships | 2,814,411 | 2,814,411 |
| Length memberships | 921,173 | 921,173 |
| Structural state facts | 14,004,908 | 14,004,908 |
| FTS blocks | not in manifest | 169,856 |
| FTS gram memberships | 56,275,908 | 56,275,908 |
| Sort entries | 566,768 | 566,768 |
| Relation occurrences | 0 | 0 |

The native manifest has corpus digest
`08cc127cb75e9f0bc54714a76beb9730a708bdc5a90d825783298a215d3c5f68`
and configuration digest
`eb03250be2bd299461b3eb11ee050a34b54e9b8ea6027f354ce574fa7991143a`.

Focused index-store tests pass (90), the full workspace test suite passes
(606 core library tests plus CLI, desktop and integration suites), workspace
all-target check passes, detailed-profiling all-target check passes, and the
benchmark Python unit suite passes (9).

The current clippy invocation completes but reports the repository's existing
large warning baseline, including warnings in untouched query-engine modules.
No warning-count comparison was used as evidence that the new paths are clean.

## Final construction measurements

Environment: Fedora Linux 7.2.15, 32 logical CPUs, rustc/cargo 1.99.1, release
build, 64 MiB redb page cache, automatic or explicitly selected construction
workers. Candidate executable SHA-256:
`1e73f616b38b40184677bcf34bb390e9764c56aaf06532477ae47aafac63b50d`.

Three fresh-process automatic-worker runs:

| Run | Complete build ns | Wall time | Post-build RSS | Process HWM | Backend length |
| --- | ---: | ---: | ---: | ---: | ---: |
| auto 1 | 26,373,192,893 | 26.74 s | 1,287,176,192 | 1,299,357,696 | 1,140,854,784 |
| auto 2 | 26,478,415,348 | 26.85 s | 1,287,385,088 | 1,299,578,880 | 1,140,854,784 |
| auto 3 | 26,526,952,975 | 26.89 s | 1,287,565,312 | 1,300,377,600 | 1,140,854,784 |
| one worker | 25,985,421,308 | 26.35 s | 1,286,811,648 | 1,299,476,480 | 1,140,854,784 |

The native measured build from the same release executable was 991,173,803 ns,
or 0.991 seconds. The automatic candidate median is 26.478 seconds: 26.71 times
the native baseline and 23.23 times the unchanged 1.15x construction limit.

Reachable application table pages total 608,202,752 bytes. The in-memory redb
backend high-water is 1,140,854,784 bytes. The final process RSS is therefore
about 146.5 MB above backend length, while backend length is about 1.88 times
the current reachable application pages. The A2 single-publication import was
570.4 MiB backend length; the A4 build's 584 publication transactions raise
backend high-water to about 1.09 GiB.

## Gate results

| Gate | Result | Evidence |
| --- | --- | --- |
| Semantic/invariant correctness | pass | zero validation errors; exact aggregate native parity |
| Construction speed | fail | 26.71x native; allowed 1.15x |
| Post-build RSS | fail | 1,287,385,088 median vs 669,508,403 allowed |
| Peak construction RSS | fail | 1,299,578,880 median vs 781,189,120 allowed |
| Memory-only operation | pass | in-memory backend only; no database file |
| Full seven-pair A4 acceptance campaign | not run | the hard multi-fold failure is already reproducible |
| Phase 7 authorization | not reached | Phase 6 remains failed |

## Architecture assessment

The failure is not primarily the prepared queue:

- Reducing the retained preparation window from 128 MiB to 32 MiB reduced the
  peak active batches from 5 to 2 but did not materially reduce RSS.
- Automatic workers and one worker have effectively the same RSS and time.
- Final prepared data is bounded to 128 MiB or less.
- Backend high-water alone is 1.140 GB.

The repeated complete-file publication policy is a leading measured cause.
Musenalm has almost one record per file, so the prescribed 256-file target
produces 584 commits. Each commit copy-on-writes affected redb pages. The
in-memory backend retains its high-water mapping; A4 does not authorize
allocator trimming, database compaction or a different publication contract.

The coordinator also remains single-core saturated. In the final profile,
admission consumes about 18.6-19.0 seconds while all worker job execution sums
to about 5.0-5.1 seconds. The application builds and serializes contributions,
then redb performs insertion and page management through the single writer.
Parsing/extraction is not the dominant wall-clock interval.

This evidence rules out accepting A4 as a localized performance repair. It
does not prove that redb is intrinsically unable to meet the target. It shows
that the current combined data model plus 584 publication transactions cannot
do so under the unchanged contracts.

## Decision required before Phase 7

1. Retain this redb design and explicitly accept measured construction and
   memory budgets of about 26.5 seconds, 1.29 GB post-build RSS and 1.30 GB
   peak RSS for ASCII Musenalm. This would require an explicit user decision;
   the current gates remain failures.
2. Reconsider the redb-primary construction/publication architecture while
   retaining the original speed and memory objectives. Concrete areas to
   evaluate are the number and lifetime of publication transactions, redb page
   reclamation/high-water behavior, and whether the required complete-file
   publication cadence can coexist with the memory model.
3. Stop the migration at the validated store/construction foundation.

Recommendation: do not start query/CRUD/frontend cutover until option 1 or 2 is
selected and measured. The work completed in A4 is correctness-valid and can
be retained independently of that decision.

## Evidence

- Corrected failed control:
  `bench/results/redb-primary-index/construction-a4/phase6-corrected/`
- Table-access candidate:
  `bench/results/redb-primary-index/construction-a4/phase6-table-access/`
- Direct-preparation diagnostics:
  `bench/results/redb-primary-index/construction-a4/phase6-direct-preparation/`
- Final candidate runs:
  `bench/results/redb-primary-index/construction-a4/phase6-final/`
- Native manifest and final full validation:
  `bench/results/redb-primary-index/construction-a4/phase6-final/native-input-manifest.json`
  and `.../ascii-validation/full-validation-current.json`
- Machine-readable gate assessment:
  `docs/ai/generated/redb-primary-index/comparison-construction-a4.json`
