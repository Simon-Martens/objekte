# Preliminary implementation verification

Run from the repository root:

```sh
cargo test -p objekte-core -p objekte-cli
cargo clippy -p objekte-core -p objekte-cli --all-targets -- -D warnings
cargo fmt -p objekte-core -p objekte-cli --check
```

| Suite | Checks |
| --- | --- |
| `configuration.rs` | Strict profile, normalization, paths, exact intended index fields, collection cycles, lazy metadata loading |
| `relationships.rs` | Nested occurrences, forward/backward edges, partial coverage, cycles, additions/deletions, missing targets, pointer escaping |
| `synchronization.rs` | Persisted hashes, equal-size/mtime edits, restart, targeted updates, partial inventories, schema/UI changes, transactions, concurrency, deleted/corrupt caches |
| `metadata.rs` | Compound manifest reuse, zero-read/zero-write reopen, candidate-only hashing, metadata-only edits, empty collections, schema/index-only work, dirty recovery and atomic manifest publication |
| `view_hashes.rs` | Component/combined fingerprints, filter permutation, resolved defaults, ordered secondary sorts, direct/SQLite parity and view-only invalidation |
| `views.rs` | Golden orders, all declared sort fields/directions, filters, pagination, projection, precise mixed numeric comparisons, null/missing, SQL parameters, generated mutations |
| CLI `cli.rs` | Real subprocesses, argument errors, table/JSON/NDJSON, cache options, reference commands, killed builder recovery |

Core unit tests also check byte-compatible buffered fingerprints across buffer
boundaries, directory/symlink replacement, and Linux directory-relative metadata
equivalence, including fractional timestamps before the Unix epoch. The unchanged
reopen test observes SQLite's `data_version` from a separate connection to ensure
that a cache hit does not commit changed database pages.

Every fixture test uses a temporary source repository and application directory.
The mutation test performs 60 deterministic add/edit/delete steps and compares four
sorts after each step against fresh direct execution. SQL and source execution use
separate filter/sort implementations with a shared schema/type contract. Golden
expected results guard against shared mistakes. The transaction failure test uses
a SQLite trigger; the interruption test actually kills the CLI during a build and
then reopens the database. Neither depends on a production fault-injection option.

## Complete Musenalm acceptance

```sh
cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
```

This intentionally longer test is excluded from routine runs. It validates all
seven collections and all 148,486 source records, compares every default-view row
and its ordering between direct execution and SQLite, exercises each configured
numeric sort and filter, counts 173,427 declared reference occurrences, checks warm
pages with zero source reads, and reopens every collection with hashes but no
reparsing of unchanged records in explicit strict mode. Default metadata reopen
checks every collection with zero source reads, and a compound numeric sort is
compared against direct execution. It uses a fresh temporary application directory.

To retain the timing/counter report:

```sh
OBJEKTE_ACCEPTANCE_REPORT=target/musenalm-library-acceptance.json cargo test -p objekte-core --release --test musenalm -- --ignored --nocapture
```

Relative report paths resolve from the workspace root. The `target` directory must
exist (Cargo normally creates it). The report records single-run acceptance timings,
not statistically controlled benchmark results. Cold runs include validation,
relationship extraction, index construction, and returning the complete collection;
warm timings return the default page. OS filesystem caching is not reset. Phase
counters ending in `_nanos` accumulate nanoseconds without per-record rounding.

The historical benchmarks have their own tests and remain buildable with
`cargo test --workspace`. Their experiments are separate from these acceptance gates.

## Profile one view across requests and sessions

```sh
cargo run -p objekte-core --release --example profile_view -- examples/musenalm contents /tmp/objekte-profile 5
```

This generic driver calls `Repository::open_view`, prints JSON lines with timings,
counters, and SQLite query plans, and checks that repeated requests and a reopened
repository return identical rows. It measures metadata loading, the first request,
further requests on the same repository handle, and one request after reopening.
Use an empty data directory to include cache construction or an existing directory
to measure verification of unchanged files. Omit `--release` to compare debug code.
An optional final filename supplies a serialized `ViewRequest` for other sorts,
filters, or pages. Timing excludes printing and result-equality checks.

The CLI exposes the same library counters with `--diagnostics`; `--output json`
also includes the query plan. Nanosecond timers have the following scopes:

- `reload_nanos`: checking configuration/schema/UI files and reloading changed metadata.
- `store_open_nanos`: cache path resolution, lock acquisition, opening SQLite,
  initialization, and saving configuration fingerprints.
- `sync_nanos`: the entire reconciliation call, including its fast reuse check.
- `fingerprint_nanos`: constructing the compound source-manifest hash, inside sync.
- `inventory_nanos`, `manifest_nanos`, `source_check_nanos`, `read_nanos`,
  `hash_nanos`, `parse_nanos`, `index_nanos`: portions of source processing.
  Source checks include opening candidates and before/after descriptor metadata; parse
  time also includes validation. Index time covers table/index DDL, not all row writes.
- `query_nanos`: query planning/execution, optional count, fetching page payloads,
  parsing the returned JSON, and constructing output rows. No terminal rendering.

For cached views, source-processing timers are **inside** `sync_nanos`; do not add
them to it. They do not account for every instruction (for example, record/edge
writes and cleanup). Initial `load_repository` and total view latency are measured
by the driver. Timings describe successful phases; a phase that errors may be only
partly accounted for. Existing-cache runs do not imply a cold OS filesystem cache.

`manifest_hits` counts the unchanged whole-manifest shortcut; `manifest_loads`
counts reading individual saved stamps from SQLite. `metadata_reused` counts
records whose source bytes were not read because their stamps were unchanged.
The default policy is `metadata-on-open`; strict full-content hashing must be
selected explicitly. View fingerprints describe definitions, not cached result
sets or proof of current source contents.
