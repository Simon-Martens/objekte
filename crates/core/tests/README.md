# Preliminary implementation verification

## Completion status

- [x] Routine core and CLI tests cover the current restricted profile, relationships, views, JSON filters, reconciliation, indexes, and validation.
- [x] Opt-in Musenalm acceptance covers all seven collections and full direct/SQLite result comparisons.
- [ ] The broader MVP's saved/grouped/relation-aware views, search, and source writes need their own future acceptance tests.

## Run

Run from the repository root:

```sh
cargo test -p objekte-core -p objekte-cli
cargo clippy -p objekte-core -p objekte-cli --all-targets -- -D warnings
python3 scripts/format-rust-spacing.py --check
```

| Suite | Checks |
| --- | --- |
| `configuration.rs` | Strict profile, normalization, paths, exact intended index fields, collection cycles, lazy metadata loading |
| `config_manifest.rs` | Persisted config diffs across restarts, shared/removed inputs, transactional ownership, applied versus observed schema versions, invalid config and failed update recovery, old-cache compatibility, no-op writes and disabled caching |
| `relationships.rs` | Nested occurrences, forward/backward edges, partial coverage, cycles, additions/deletions, missing targets, pointer escaping |
| `synchronization.rs` | Persisted hashes, equal-size/mtime edits, restart, targeted updates, source changes during reconciliation with rollback and recovery, partial inventories, schema/UI changes, transactions, concurrency, deleted/corrupt caches |
| `metadata.rs` | Compound manifest reuse, zero-read/zero-write reopen, candidate-only hashing, metadata-only edits, empty collections, schema/index-only work, dirty recovery and atomic manifest publication |
| `view_hashes.rs` | Component/combined fingerprints, filter permutation, resolved defaults, ordered secondary sorts, direct/SQLite parity and view-only invalidation |
| `json_parsing.rs` | Sonic duplicate-key behavior across views/indexes/relationships, opaque precise numbers, and malformed unselected cached payloads |
| `json_filter.rs` | Typed filters, field operands/modifiers, LIKE patterns, exact numbers, nested arrays, missing/null, parser limits, injection-shaped bindings, global scope, source changes, and direct/SQL parity |
| `expression_indexes.rs` | Real JSON expression-index seeks, covering-key reads with extraction disabled, lazy collection tables and view/query indexes, no field columns/value tables, format-5 rebuilds, precise types, edits/deletions, reconfiguration, and restart/rebuild |
| `migrations.rs` | New cache schema, adoption of existing format-6 caches, one-time application, transactional retry after a failed schema step, and rejection of future migration history |
| `ingestion.rs` | Serial/parallel parity across batches and oversized records, exact reference occurrences, targeted updates, retained content errors across batches and correction/revalidation |
| `validation.rs` | Pending/restart recovery, wrong types, malformed JSON/UTF-8, all diagnostics, stale-result guards, schema changes, delete/recreate, background transaction failure, worker defaults/stacks, shared writer concurrency |
| `views.rs` | Golden orders, all declared sort fields/directions, filters, pagination, projection, precise mixed numeric comparisons, null/missing, SQL parameters, generated mutations |
| CLI `cli.rs` | Real subprocesses, two clients contending for one cache lock, timed SQL diagnostics without SQL text in result JSON, argument errors, table/JSON/NDJSON, cache options, validation modes/statuses, reference commands, killed builder recovery |

Index unit tests exercise seeded keys, duplicate inputs, malformed values, zero-budget
fallback, and cleanup after successful and failed builds. The production ingest
batch selector has boundary tests for 256 records, exactly 8 MiB, and a lone
oversized record; `ingestion.rs` checks the resulting rows and edges.

Core unit tests also check byte-compatible buffered fingerprints across buffer
boundaries, directory/symlink replacement, and Linux directory-relative metadata
equivalence, including fractional timestamps before the Unix epoch. The unchanged
reopen test observes SQLite's `data_version` from a separate connection to ensure
that a cache hit does not commit changed database pages.

Unit test modules live beside their source files as `*.tests.rs`. Each production
module includes its test file with `#[cfg(test)]` and `#[path = "name.tests.rs"]`;
the Unix-only source tests retain their platform guard.

Run `python3 scripts/format-rust-spacing.py` after editing Rust. It runs rustfmt,
then keeps one empty line between functions and two around struct, enum, impl,
and module blocks.

Sonic parser unit tests compare values and serialized bytes against Serde for
large numbers, exponents, negative zero, Unicode, duplicate/escaped keys, and the
existing nesting limit. Projection tests cover missing/null fields, large skipped
payloads, malformed skipped JSON, and trailing input. Skipped fields use Sonic's
native skip behavior, without an extra pass to validate surrogate escapes; a test
checks that unselected surrogate escapes do not trigger reparsing. The public/schema
value representation and fingerprint serializer remain
unchanged. Page parsing materializes only selected fields; its work is included in
`query_nanos`, while `parses` counts full source/cache revalidation parses.

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
cargo test -p objekte-core --release --test musenalm_json_filter -- --ignored --nocapture
```

This intentionally longer test is excluded from routine runs. It validates all
seven collections and all 148,486 source records, compares every default-view row
and its ordering between direct execution and SQLite, exercises each configured
numeric sort and filter, counts 173,427 declared reference occurrences, checks warm
pages with zero source reads, and reopens every collection with hashes but no
reparsing of unchanged records in explicit strict mode. Default metadata reopen
checks every collection with zero source reads, and a compound numeric sort is
compared against direct execution. It uses a fresh temporary application directory.
The JSON filter acceptance test compares complete direct and SQLite results across
the global filters and the scoped `contents` filter, then checks every scoped SQL
page against the source result, including an empty page after the end.

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

The CLI exposes the same integer counters in JSON results and with `--diagnostics`,
converting timers to floating-point milliseconds and renaming their suffix to `*_ms`
(for example, `sync_ms`). This includes view, cache-refresh, and validation results.
Library metrics retain nanoseconds; JSON view output also includes the query plan.
Only metric fields are converted; similarly named source-record fields remain
unchanged. Library timers have the following scopes:

- `reload_nanos`: checking configuration/schema/UI files and reloading changed metadata.
- `store_open_nanos`: cache path resolution, lock acquisition, opening SQLite,
  and initialization.
- `configuration_nanos`: reading/comparing the persisted input manifest, verifying
  loaded configuration, and atomically committing changed digests/consumers.
- `filter_compile_nanos`: parsing/binding a general JSON filter and computing its definition hash.
- `sync_nanos`: the entire reconciliation call, including its fast reuse check.
- `fingerprint_nanos`: constructing the compound source-manifest hash, inside sync.
- `inventory_nanos`, `manifest_nanos`, `source_check_nanos`, `read_nanos`,
  `hash_nanos`, `parse_nanos`, `index_nanos`: portions of source processing.
  Source checks include opening candidates and before/after descriptor metadata; parse
  time includes validation only for direct/cache-disabled execution. Index time covers expression-index construction and statistics; SQLite maintenance during record writes is included in record write time.
- `json_decode_nanos`, `validation_nanos`: decoding/conversion and schema/numeric-domain
  validation, respectively, inside `parse_nanos`; do not add these to that parent.
- `parallel_prepare_nanos`: wall time creating the private pool and preparing bounded
  batches for an empty collection cache. Per-file read/hash/decode timers
  are summed across workers and overlap in wall time. Do not add them to this timer
  or use their sum as elapsed build time. The surrounding `sync_nanos` is wall time.
- `record_write_nanos`: looking up the previous digest and inserting/updating the
  source payload and metadata in `put_record`, including preparation of temporary
  build keys or SQLite's maintenance of existing expression indexes.
- `relation_extract_nanos`, `relation_write_nanos`: extracting reference occurrences
  for `put_record`, then comparing and updating persisted outgoing edges.
- `index_rows_written`: source records feeding newly built expression indexes or
  changed records whose existing indexes SQLite maintains; no separate value rows
  are written. The former `index_row_write_nanos` timer was removed with those writes.
- `commit_nanos`: publishing the reconciled collection state and committing its
  transaction, including any SQLite work triggered by that commit.
- `query_nanos`: query planning/execution, optional count, fetching page payloads,
  parsing the returned JSON, and constructing output rows. No terminal rendering.

For cached views, source-processing timers are **inside** `sync_nanos`; do not add
them to it. They do not account for every instruction (for example, deletion,
metadata-only updates, and cleanup). Primary-key and existing secondary-index
maintenance is included in row/edge write times, not `index_nanos`. Initial
`load_repository` and total view latency are measured
by the driver. Timings describe successful phases; a phase that errors may be only
partly accounted for. Existing-cache runs do not imply a cold OS filesystem cache.

`manifest_hits` counts the unchanged whole-manifest shortcut; `manifest_loads`
counts reading individual saved stamps from SQLite. `metadata_reused` counts
records whose source bytes were not read because their stamps were unchanged.
The default policy is `metadata-on-open`; strict full-content hashing must be
selected explicitly. View fingerprints describe definitions, not cached result
sets or proof of current source contents.

Background validation metrics are independent: `snapshot_read_nanos` reads bounded
cached snapshots; `parallel_validation_nanos` measures worker wall time;
`decode_nanos` and `validation_nanos` sum CPU work across workers; `write_nanos`
includes acquiring the shared writer and conditionally publishing results. The CLI
also logs `validation_wait_ms`. Do not add nested/overlapping timers together.
Full Musenalm acceptance now waits for background completion and asserts that every
record is valid, as well as comparing complete ordered outputs and relationships.

## Compare index revisions

`crates/core/examples/profile_indexes.rs` runs a JSON workload through the library,
checks repeated/reopened results, and emits all phase durations in milliseconds.
`crates/bench/index-workload.json` covers all seven Musenalm default views, numeric
sorting/filtering/counting, compound sorts, offsets, scoped/global JSON queries,
wildcard searches, and nested-array queries.

Build the same driver against each revision (copy it into older checkouts first),
then run:

```sh
python3 crates/bench/compare_index_revisions.py /path/to/before-profile /path/to/after-profile \
  examples/musenalm crates/bench/index-workload.json target/index-comparison \
  --samples 3 --repeats 3
```

The runner alternates revision order, creates a fresh temporary database for each
sample, and compares result digests across both revisions and every repetition.
It saves raw JSON lines and median first/warm/reopen timings. Do not run competing
CPU/I/O workloads alongside a timing comparison. Correctness tests assert actual
index use; they avoid unstable wall-clock thresholds.
