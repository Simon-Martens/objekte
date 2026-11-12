# Extended capabilities increase snapshot size and process RSS

Category: `perf`. Originating task: `staged-index-construction`. Relevant
revision: `21265636a1b7` on `integration/staged-index-construction`;
`crates/core/src/field_state.rs`, `lifecycle.rs`, `repository.rs`, and
`storage.rs` own the additional state, batch publication, and persistence.

The matched Musenalm ASCII run at the prerequisite `b19df1ed4a38` produced a
129.14 MB full snapshot and reported 533.78 MB resident process memory after
build. At the measured staged revision, the base snapshot is 131.03 MB;
configured complete is 165.40 MB with 6.76 MB estimated retained compact
states; explicit inference is 218.63 MB with 21.16 MB estimated compact
states. The staged process reported 545.3 MB RSS at base, 928.9 MB at
configured complete, and 1,149.4 MB at inferred complete. RSS includes
allocator retention and benchmark process history; it is not a direct live
index-byte count. The Unicode phase runs second in each process and should
not be compared as an independent fresh-process RSS sample.

The previous full build took 2.901 s ASCII. With one default background
worker reserved to protect foreground latency, configured staged work reaches
SearchReady at 2.128 s and finishes optional work at 6.638 s; explicit
inference finishes at 11.255 s. The second parse of 148,486 records is an
accepted cost of the assigned two-pass architecture. The worker limit is a
measured responsiveness/throughput choice, not a claim of equal total build
time. The newly retained compact states and inferred postings explain part
of the snapshot increase; the remaining bytes also include immutable batch
coverage and extended structures. The precise breakdown of allocator-retained
RSS remains unproven. Completed-batch compaction was implemented within the
task before these final measurements, reducing unnecessary batch metadata.
The practical impact is more snapshot disk space, higher observed process RSS,
and longer time to full optional coverage. Further representation work was not
attempted after the in-scope compaction because it would widen the persistence
and publication design; the measured base and query correctness gates pass.

Reproduce with the predecessor `bench_musenalm --extended` and staged
`bench_staged_musenalm --mode base|complete` (plus `--inference`), with
`--snapshot`, ASCII profile, four query workers, and the same Musenalm corpus.
Inspect `index-results.json` and `snapshot-results.json` under
`bench/results/staged-index-construction/`. The expected future outcome is a
smaller retained/persisted footprint at the same exact coverage and query
behavior. Potential approaches are further batch/coverage encoding and
measuring live allocations separately from allocator high-water RSS; these
are hypotheses for separate review, not authorized changes under this issue.
