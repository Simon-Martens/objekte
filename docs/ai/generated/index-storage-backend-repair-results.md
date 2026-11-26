# Index storage backend repair release comparison

Measured 2026-11-26 on Linux x86-64 with an Intel Core i9-13900K, 32
available hardware threads, Rust 1.100.0 and the default Cargo release
profile. Both revisions used the same lockfile build command and workload
options. The build and query replay used the repository's Musenalm dataset
with 148,486 records. The stress benchmark ran on a fresh writable copy of the
data and collection definitions for each run. The build benchmark resolved
auto to 32 workers.

- Master `cf7253d048c9d6fe5d2a7e9b47441b2207cce932`
- Repair `47d17ef5042947066a113e9b53f93ce81796b742`

## Paired runs

The first-row measurement uses `residual_order_cases` from
`crates/cli/examples/bench_query_execution_addendum.rs`. Its 1,000-row JSONL
fixture uses a stored ascending `score` order, `body != 'miss'`, byte delivery,
evidence, a limit of one, complete counting disabled and a disabled result
cache. Each run warms the query once and measures 64 calls to the ordered
stream's first nonempty event in the same process. The eight-file measurement
uses `bench_lifecycle_mutations stress` and takes its `batch_small_8` row. The
batch timer covers the eight update calls after the fixture paths and handles
have been prepared.

| Pair | First revision | Master first row (µs) | Repair first row (µs) | Master batch (ms) | Repair batch (ms) |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | master | 24.401 | 24.241 | 4.706 | 3.581 |
| 2 | repair | 23.402 | 25.227 | 4.655 | 3.052 |
| 3 | master | 23.867 | 24.087 | 5.689 | 3.468 |
| 4 | repair | 23.977 | 24.297 | 4.850 | 6.198 |
| 5 | master | 24.096 | 23.692 | 4.791 | 5.609 |
| 6 | repair | 23.887 | 23.893 | 4.720 | 2.986 |
| 7 | master | 23.924 | 24.553 | 2.595 | 4.219 |
| Median | | 23.924 | 24.241 | 4.720 | 3.581 |

The first-row median rose 1.3%. The eight-file batch median fell 24.1%,
with faster repair runs in four of the seven pairs. Six first-row pairs
were within 1 µs. The previously reported first-row medians differed by
21.8 µs. The median difference here was 0.32 µs, and the work probes
leave its cause uncertain.

## Work before the first row

Both revisions parsed and verified one record, read 26 source bytes, and
emitted one row. Allocation counts use separately run allocator hooks through
the first ordered-row callback. The directory probe counts storage-level group
acquisition, physical placement pages and point reads. The master accesses a
resident placement vector in the runner. The probe records calls to its
`IndexView::segments` helper separately.

| Per first row | Master | Repair |
| --- | ---: | ---: |
| Allocation or reallocation calls, median | 176 | 154 |
| Placement group acquisitions | 0 | 1 |
| Physical placement pages read | 0 | 0 |
| Point placement reads | 0 | 0 |
| Live bitmap requests | 3 | 2 |
| Live bitmaps built from a range or paged mask | 3 | 0 |
| Resident live bitmap clones | 0 | 2 |

## Work in one eight-file update batch

Separate release probes measured the publication path. Preparation covers file
preparation through the returned draft. The post-preparation interval includes
index staging, revalidation and publication. Counts below cover the timed calls.
The policy rows are placement visits across complete passes over the selected
collection's group. Repeated visits to the same placement are included.

| Per batch | Master | Repair |
| --- | ---: | ---: |
| Catalog metadata point calls in the view | 40 | 40 |
| Catalog entry gets | 48 | 56 |
| Catalog rows replaced | 8 | 8 |
| File-count route placements visited | 8 | 8 |
| Native point placement reads | 0 | 24 |
| Placement group acquisitions | 0 | 16 |
| Placement policy invocations | 16 | 16 |
| Complete passes over selected placement groups | 48 | 48 |
| Rows visited during those passes | 672 | 672 |
| Repository-wide directory enumerations | 0 | 0 |
| Catalog path enumerations | 0 | 0 |
| Physical placement pages read | 0 | 0 |
| Prepare (ms) | 0.852 | 0.908 |
| Stage, revalidate and publish (ms) | 4.489 | 5.155 |

Post-preparation work dominated the instrumented run. The timed pairs have
substantial spread, so a single instrumented run cannot attribute the paired
batch timings to any particular operation.

## Build and result replay

```sh
cargo build --locked --release -p objekte-cli --example bench_build_musenalm --example bench_musenalm --example bench_lifecycle_mutations
bench_lifecycle_mutations stress <writable-fixture-copy>
bench_build_musenalm --root examples/musenalm --alphabet ascii --build-workers auto
bench_musenalm --root examples/musenalm --profiles ascii-off --repetitions 1 --result-dir <output>
```

The stress invocation used `OBJEKTE_BENCH_MUTATE_COPY=1` on the fixture copy.
The representative build and replay use one run per revision.

| ASCII auto-worker build | Master | Repair |
| --- | ---: | ---: |
| Build (ms) | 722.50 | 671.53 |
| Wall time (ms) | 735.45 | 695.83 |
| Peak RSS (MiB) | 665.00 | 613.81 |
| Parsed records | 148486 | 148486 |

All 150 result digests match, as does the build index signature. The
[raw samples](index-storage-backend-repair-results.json) include the 64 first-row
times per run, batch timings, allocation samples, and the separate probe counts.
