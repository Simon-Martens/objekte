# Staged index construction: integrated implementation and results

Task: `staged-index-construction`
Prerequisite/master base and measured predecessor: `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`
Unified planner completion: `a68cb0c67b68987251d35f528ccbbe3445509ea1`
Predicate ordering/shared evaluation completion: `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`
Measured implementation: `21265636a1b7` on `integration/staged-index-construction`
Cancellation timer added at `26e090ff`; it does not change index behavior.
Integration worktree: `/home/simon/source/objekte-integration-staged-index-construction`
Date: 12 November 2026

## Scope and public behavior

The completed unified planner and predicate-ordering/shared-evaluation work
were present in the prerequisite commit. The new implementation uses that
planner for compact state and partially covered capabilities; it does not add
an application-specific query engine.

| Surface | Behavior |
| --- | --- |
| Core | `Index::start_staged_build` returns a task with `catalogue`, `status`, `wait_search_ready`, `wait_complete`, pause/resume/cancel. `Index::view` pins a data/capability snapshot. `Index::start_inference` requests later explicit discovery. `Index::build` remains blocking through the same staged path. |
| CLI | Queries and TUI enter at SearchReady by default. `--index-wait=complete` waits for selected field work. Index-only `--auto-index` and explicit full builds wait for requested completion. Query exit quiesces optional work; requested snapshot saves retain a coherent partial or complete state. Progress stays off machine-readable stdout. |
| Desktop | The native Open flow attaches the discovered file catalogue, then enables queries at SearchReady. The Index panel shows extended status, Details, and explicit **Infer field indexes**. Capability publication preserves current results, focus, scroll, and tabs. |

SearchReady means a query can execute against the complete validated base
domain. It does not mean every optional index is ready or that an unavailable
source file can be read. An unindexed or uncovered predicate uses source
evaluation under the existing error contract.

| Work | First pass | Second pass |
| --- | --- | --- |
| Discovery and identities | Discover membership, stable file IDs, record boundaries and locators | Reuse the published catalogue; no rediscovery |
| Correctness | Declared/schema and format validation, logical versus raw fallback, indispensable relation/identity facts | Reject changed source layout and leave unresolved coverage on failure |
| Text | Configured text extraction and bigrams | Reuse unchanged text/bigrams |
| Optional fields | No optional typed, length, sort, inference, or compact state work | Configured postings and sort keys, explicit inference, compact states, in one bounded traversal |
| Adapter cost | A registered source adapter may still decode a logical value for validation/text extraction, but the value is released within its work unit | Reread and parse applicable records once per normal second pass; JSONL stays file/batch grouped |

An unconfigured, inference-disabled collection skips the second pass. With a
configuration and inference off, only fields selected by that configuration
receive compact states and optional postings. Explicit inference discovers
eligible nested fields and sub-fields and builds their compact states; it
retains existing value-dictionary admission limits and explicit exclusions.
Declared `indexed: false` remains an opt-out from inferred value postings.
Queries to any field remain valid regardless of selection.

## Predicate and coverage contracts

`field:?`, `:string`, `:number`, `:array`, `:null`, and `:object` are Boolean
predicates with `=` and `!=` against Boolean literals. `:exists` is preserved.
`:?` is true when any selected value is nonempty. Absence, null, empty string,
empty array, and empty object are empty; zero and false are nonempty. The type
tests accept the corresponding logical type, including empty containers and
strings. On multivalued selections, a positive test is existential and a
false comparison negates the aggregate positive result. A record may satisfy
both `:string=true` and `:null=true`. A terminal array is different from its
selected elements. Existing selector and correlated-element rules constrain
when a record-level state summary can prove a compound expression. Duplicate
object keys use the adapter's logical last-key resolution. Existing
conversions and escaped field names retain their grammar; invalid chains and
non-Boolean comparisons fail at binding. No `()` state was added. See
[FILTER grammar](../../filter-grammar.md).

Each selected canonical path has a covered-record domain and an 11-class
occurrence mask. A 256-slot block packs absence/single classes into 4-bit
codes, stores sparse mixed masks separately, and switches to a dense mask
block for heavily mixed data. Entirely absent blocks need no state allocation.
The mask retains null, false/true, zero/other number, empty/other string,
empty/other array, and empty/other object independently. Compact states
replace equivalent permanent presence/null/empty-string facts where exact;
length postings remain for nonzero lengths. A compact state cannot prove
arbitrary scalar ranges, array cardinality, transformed equality, or
same-element correlation.

Each completed extended batch publishes immutable data with its exact covered
record IDs, config/data identity, capability revision, and diagnostics. The
planner combines exact and conservative `must`/`may` bounds with the existing
AND/OR/NOT evaluator, then verifies unresolved survivors from source.
Estimate-only EXPLAIN does no source I/O. A query, count, continuation, or
export keeps one pinned data/capability view; a capability-only publication
does not invalidate semantic result membership. Data mutations prune stale
batches and invalidate affected caches. Save/load persists partial coverage
and pending policy; compatible restore is searchable before remaining work
resumes. Pause, cancellation, resource failure, and shutdown preserve the
published base and distinguish optional work failure from query failure.

## Measurement setup

The predecessor is the actual completed prerequisite at `b19df1ed4a38`.
Musenalm's extended suite has 194 predecessor cases plus eight new state
cases. The corpus fingerprint is
`259b435df73c0eb2d1d4f066f1528f9dc63c14b29766d19798078fdcd569b473`.
MB below denotes decimal millions of bytes.
ASCII/Unicode alphabets, verified-result cache off/64 MiB warm, four query
workers, 16 detected base-build workers, one default extended worker, 16 MiB
prepared cache, and three repetitions produce 2,424 samples
per staged readiness mode. The baseline has 2,328 samples (194 cases), all
successful. Release binaries used Rust 1.99.1 on an AMD Ryzen 7 5700G
(8 cores, 16 logical CPUs), 30 GiB RAM, Fedora Linux 7.2.13. Runs were
serialized. The normal suite used the host filesystem cache as found; its
second pass is a warm read, not a cold-storage promise. The comparison script
checks definitions, result IDs, evidence, order, counts, settings, and
latencies against the predecessor.

Commands, with each `MODE` substituted by `base`, `partial`, `complete`, and
`complete --inference` respectively:

```sh
target/release/examples/bench_staged_musenalm --mode MODE \
  --snapshot --query-workers 4 --verified-cache-bytes 67108864 \
  --repetitions 3 --result-dir target/staged-index-construction-final5-MODE
python3 bench/compare_staged_results.py \
  bench/results/staged-index-construction/baseline-suite \
  target/staged-index-construction-final5-MODE --output comparison.json
```

The partial mode used `--snapshot-resume` in place of `--snapshot`. All raw
manifests, sample timings, result digests, query counters, build/readiness
measurements, snapshot sections, and comparisons are retained under
[`bench/results/staged-index-construction/`](../../../bench/results/staged-index-construction/).

## Readiness, construction, and storage

| Mode | First listing ASCII/Unicode | SearchReady ASCII/Unicode | Optional completion ASCII/Unicode | Build parses |
| --- | ---: | ---: | ---: | ---: |
| Predecessor full build | Not instrumented | 2.901 / 2.575 s full build | Same full build | 148,486 |
| Staged base, optional work paused | 2.49 / 2.70 ms | 2.117 / 2.214 s | Not requested | 148,486 |
| Staged partial | 2.47 / 2.66 ms | 2.159 / 2.227 s | Stopped after 512 of 148,486 files | 148,486 at query measurement |
| Staged configured complete | 2.43 / 2.64 ms | 2.128 / 2.140 s | 6.638 / 6.646 s | 296,972 |
| Staged explicit inference complete | 2.43 / 2.67 ms | 2.121 / 2.218 s | 11.255 / 11.292 s | 296,972 |

The first text page became available at 2.144/2.241 s for base,
2.159/2.172 s for configured complete, and 2.151/2.250 s for inference
(ASCII/Unicode). This includes the SearchReady build; the first Q001 page
itself took about 27–32 ms. SearchReady varied by tens of milliseconds
between modes on this host. The first listing callback preceded SearchReady
by about 2.1–2.2 s. The one-worker optional budget made configured and
inferred completion slower than the predecessor's full build while keeping
queries responsive during that work. The paired seven-repeat check below
separates host/cache variation from these single full-suite build samples.

The first paired warm run exposed a real implementation regression: the
predecessor's seven-run full-build median was 2.545 s while the staged base
took 2.776 s. The discovery callback copied the entire growing catalogue at
every progress publication. Commit `21265636` changed it to append only new
entries and added an exact final-catalogue test. In seven immediately paired
warm runs after that fix, SearchReady was 2.126 s median (IQR 0.025 s) versus
2.554 s (IQR 0.019 s) for the predecessor's full build, about 17% faster.
The completed final suite uses that fix. A second seven-repeat series at the
pushed revision confirmed it under both warm and advisory-evicted source
pages:

| Condition | Predecessor full build median (IQR) | Staged SearchReady median (IQR) | Configured/inferred completion median (IQR) |
| --- | ---: | ---: | ---: |
| Warm filesystem cache | 2.552 s (0.022) | 2.133 s (0.036) | 6.644 s (0.024) / 11.204 s (0.022) |
| Advisory data-page eviction before each run | 2.705 s (0.020) | 2.272 s (0.024) | Not measured |

`bench/evict_musenalm_pages.py` requested `POSIX_FADV_DONTNEED` on 148,500
files totalling 454,176,234 B before each paired evicted run; it reported
zero errors. This requests clean file-data page eviction but does not reset
filesystem metadata caches or prove physical cold reads. The warm and
evicted series were kept separate; the latter is an advisory cold-data
experiment, not a cold-storage guarantee.

The predecessor full build was 2.901 s ASCII and 2.575 s Unicode, with
148,486 record parses. Its estimated index sizes were 69.66/72.57 MB and its
snapshots were 129.14/131.65 MB. A staged base has the same 148,486 first-pass
parses and no selected field states. Configured or inferred complete has
296,972 parses, exactly one additional normal pass. The second pass is an
intentional total-work cost in exchange for earlier search readiness.

| ASCII profile | Snapshot | Estimated retained compact states | Process RSS at measurement |
| --- | ---: | ---: | ---: |
| Predecessor full build | 129.14 MB | No compact states | 533.8 MB |
| Staged base | 131.03 MB | 0 | 545.3 MB |
| Staged partial, 512 files | 131.25 MB | 0.072 MB | 564.7 MB |
| Staged configured complete | 165.40 MB | 6.760 MB | 928.9 MB |
| Staged inferred complete | 218.63 MB | 21.160 MB | 1,149.4 MB |

The Unicode snapshots were 133.54 MB base, 133.76 MB partial, 167.91 MB
configured, and 221.14 MB inferred. Unicode ran second in each process, so
its RSS includes allocation history from the ASCII phase and is not an
independent fresh-process memory comparison. Even the ASCII RSS is a process
measure, including allocator retention; compact-state bytes are the separate
live allocation estimate. The configured complete snapshot's lifecycle
section was 37.98 MB. Completed-batch compaction reduced this section during
implementation, but persisted and process footprints remain materially above
the predecessor. [The footprint issue](../issues/perf/staged_extended_snapshot_and_memory_footprint.md)
records the tradeoff and the unproven portion of the RSS increase.

A saved partial snapshot restored in 0.991/0.926 s, was searchable at its
published capability revision, and resumed to complete in 4.449/4.483 s
(ASCII/Unicode) without redoing base text construction. Its final resumed
phase was `Complete` with all 148,486 files covered.

The completed configured pass advanced capability revision 1→296, or 295
batch publications over about 4.51 s of optional work (roughly 65/s).
Catalogue progress publishes after the first file, each 4,096 discoveries,
and completion: at most 38 publications for this corpus. These are
publication counts, not measured UI repaint rates. Lock-hold nanoseconds
and peak per-batch temporary allocations were not instrumented; the RSS and
retained-state numbers above do not substitute for those measurements.

The seven-repeat synthetic shape fixture has 64 JSONL files, 16 records each,
sparse nested fields, repeated mixed types, and 128 rotating dynamic fields.
The unconfigured base parses 1,024 records and retains no field-state
columns; inferred completion parses 2,048 and retains 136 columns with
139,264 coverage cells and 1,024 mixed exceptions. Their encoded columns
occupy 367,072 B and estimated retained state is 438,923 B; the incremental
snapshot is 551,134 B (133,465 B base, 684,599 B inferred). All eight
representative predicates have identical result identities. Base execution
parses 1,024 source records per query; inferred execution parses zero for
these covered predicates. The fixture is a shape/stress comparison, not a
claim that compact state always shrinks total index bytes.
Across seven runs at the final code, median SearchReady was 5.329 ms in base
mode and 5.472 ms with inference requested; inferred completion was 23.697
ms median. SearchReady therefore did not wait for optional discovery in this
fixture.

## Query latency and regressions

| Readiness mode | 202-case samples | Correctness mismatches | Timing flags above 5% and 0.5 ms |
| --- | ---: | ---: | ---: |
| Base, paused | 2,424 | 0 | 196 |
| Partial | 2,424 | 0 | 192 |
| Configured complete | 2,424 | 0 | 54 |
| Explicit inference complete | 2,424 | 0 | 3 |

The flags are case/profile rows, not failures; the predecessor has 194 cases
and the eight new predicate cases Q195–Q202 passed at every readiness mode.
Early-mode flags mostly reflect required source fallback, including Q136:
its 133,948-record numeric range and sort takes several seconds without
configured coverage and 45.8 ms when configured work completes, versus 56.6 ms
in the predecessor. At configured completion, the largest remaining flags
are unselected structural paths. Q108 went 0.233→46.025 ms with 4,010
source parses, Q116 went 10.580→369.833 ms with 133,948 parses, and Q120
went 8.440→342.533 ms with 133,948 parses (ASCII cache off). All three
became source-free under explicit inference. This is a real performance cost
of exact field selection. It is recorded in the
[unselected-field issue](../issues/perf/staged_unselected_structural_field_fallback.md),
with the possible configuration/inference remedies.

The three inferred-complete flags were small text-query differences:
for example Q001 Unicode cache off was 25.97→28.26 ms. Earlier matched
full-suite runs at the previous implementation commit produced 14–69 inferred
flags and different source-scan hot spots, although source-file, byte, and
record-parse counters matched. A later paired seven-repeat probe showed
slower broad scans, while a further no-context experiment changed the
ASCII/Unicode direction without improving both. The evidence supports a
remaining host/cache-sensitive latency limitation; it does not establish a
single stable evaluator defect. The candidate-only staged proof and
resident-cache paths were improved during this task, eliminating larger
repeatable regressions in Q054, Q134/Q135, Q178, and Q191. No query result
was changed to improve timing.

The optional worker budget was tuned after a focused seven-repeat foreground
check showed queries contending with pass 2. Default optional work now uses
one build worker unless explicitly overridden. With that worker and the
extended task still `Building`, focused medians were:

| Query, cache off | Paused base ASCII/Unicode | Foreground ASCII/Unicode | Overlap samples |
| --- | ---: | ---: | ---: |
| Q001 text page | 14.83 / 13.97 ms | 16.70 / 16.39 ms | 7 / 7 |
| Q006 scan | 310.35 / 296.36 ms | 353.08 / 347.85 ms | 7 / 7 |
| Q007 scan | 368.59 / 351.69 ms | 427.94 / 425.83 ms | 6 / 7 ASCII, 7 / 7 Unicode |

The remaining 13–21% slowdown is visible during shared disk/cache work. The
earlier unrestricted worker setting roughly doubled Q001 ASCII in the same
focused method (29.35 versus 14.71 ms). One worker reduces foreground
contention at the cost of longer optional completion; callers can override
the worker setting. The foreground status log records 42 of 98 samples
starting while extended work had remaining files, and every query succeeded.

Seven independent one-query process trials timed `Index::quiesce()` while the
extended task was still active. Each finished in phase `Cancelled` with
3,584 of 148,486 files completed. Median quiesce duration was 35.47 ms
(inclusive IQR 32.97–37.68 ms; range 31.63–41.77 ms). This measures the
blocking request-and-join operation at a safe batch boundary, not a native
pointer-to-frame latency. The exact trial results and runner manifests are
under `bench/results/staged-index-construction/cancellation-final/`.

## Validation, native evidence, and delivery

`cargo test --workspace --locked` passed at the final code source: 850 passed,
0 failed, 17 ignored across 22 test groups. This includes 528 core library
tests, 180 desktop library tests, 36 CLI process tests, the new catalogue
append regression test, staged coverage, snapshot/load/resume, mutation,
inference, and query-order checks. `cargo clippy --workspace --all-targets
--locked` passed with existing unused-code warnings. `git diff --check`
passed. The repository-wide spacing formatter/check was attempted; its
`--check` still fails at the unchanged `crates/core/src/storage.test.rs`
blank-line pattern and would rewrite many unrelated files. Those formatter
edits were discarded; the task-owned diff is whitespace-clean.

The [native desktop report](staged_index_construction_desktop_validation.md)
records a launched GPUI app under Sway, a three-record Open and type filter,
30,000-file catalogue, explicit live inference, Details, a narrow light
window, keyboard preview, and screenshots. It notes that the catalogue-only
frame and pointer cancellation were too brief for a native capture; core
tests cover their lifecycle behavior.

Subtasks merged from `task/staged-index-construction/field-states` (starting
implementation `0ada21b8`), `task/staged-index-construction/construction`
(final merged checkpoint `b91bca5c`), `task/staged-index-construction/cli`
(`b222c7e4`), and `task/staged-index-construction/desktop` (`8901f688`).
Integration fixes cover computed-sort cache correctness (`01d3fafa`), staged
proof planning (`df3571fd`, `c94cedf4`), later inference/compaction
(`b91bca5c`), and foreground capacity (`fdff7185`).

The original checkout remained on `master` at the prerequisite commit while
work proceeded in the integration worktree. No task code was merged into
master. The completed specification and implementation report are at
[`docs/ai/done/objekte_staged_index_construction_specification.md`](../done/objekte_staged_index_construction_specification.md)
on the pushed integration branch. The branch and worktree are retained for
the user's explicit merge command.
