# Keep source-format policy outside Objekte core

Implement this assignment from merged master `31b3ed8c22aee12bc3972e01be0ed3e93480a1a8`. Use `integration/format-independent-core` and the sibling worktree `/home/simon/source/objekte-integration-format-independent-core`. Follow `docs/ai/README.md`. Push the integration branch for review.

## Completion criterion

An application depending only on `objekte-search-core` can construct typed records, build filters, parse query literals and search. Core treats every file reader as a registration with an opaque identity and declared capabilities. Remove the built-in format selectors `Json`, `JsonLines` and `JsonDecodedScalars`, document readers, format policies and source inclusions from either loader crate. Move the document and transport uses of `serde_json` to their owners and remove the direct core dependency as specified below.

**Name helpers for the syntax they implement.** A core helper that unescapes one JSON string must use an explicit name such as `unescape_json_string`. A helper that writes a JSON string literal should be named `write_json_string_literal`. Use equally explicit names for their dedicated modules and tests. A filter constructor accepting an already-typed value can use a format-neutral name. A caller can search records from any source and still use JSON escape rules for a quoted query argument. Whole-document conversion, wire-format parameter decoding and schema-file syntax belong outside core. Keep format-independent Serde derives.

## 1. Reader selection and record identity

Replace the closed variants in `source/mod.rs` with a format-neutral reader identifier. Keep the existing registry lookup and resolve registrations once during configuration. Have the loader or host supply the default identity policy to folder construction. Core already has a reader capability for validating relative-path identity. Keep validation separate from choosing the default. A generic registration can retain generated IDs even if its reader supports relative-path IDs.

| Caller configuration | Behavior to preserve |
|---|---|
| Ordinary host folder using the JSON document reader or its decoded-scalar representation | Relative-path record IDs, including the extension |
| JSON Lines folder | Generated record IDs by default |
| Generic registration | Generated IDs by default. Registering a reader under a familiar string grants no capabilities |
| Explicit collection identity | Honor the configured filename, relative-path, generated or intrinsic policy subject to the existing reader checks |

Keep `json`, `json_decoded_scalars` and `jsonl` as stable **external** registration IDs. Preserve the CLI/TUI and desktop defaults, repository configuration, explicit schema mode and independent loading of either library. Update callers of the removed core API and give downstream Rust users a short old-to-new mapping.

**Preserve configuration fingerprints.** Both hash functions in `repository/configuration/identity.rs` currently feed the debug spelling of the format and identity policy into SHA-256. The old built-in selector and `Registered("json")` can resolve to the same reader and still contribute different bytes. Capture baseline cases for both selectors and both hash functions. A concrete option is to carry the old selector encoding as opaque compatibility data on the selector created by the loader or host. Keep the generic registration encoding for existing generic callers. Do not make the registry ID alone select a legacy encoding. Keep the existing four fixed hash expectations.

## 2. Values, query syntax and conveniences

Move recursive `serde_json::Value` conversion and JSON number adapters from `model/json.rs` to `objekte-loader-json` as free functions or a loader-owned extension trait. Update the document, projection and borrowed-scan call sites. Keep core's numeric representation, comparison and scalar literal conversion authoritative. Reuse the existing decimal grammar in `model/decimal.rs`. Preserve integer fast paths, finite-float behavior, exact-decimal fallback and resource limits. Keep parse-status diagnostics meaningful.

Move `parse_parameter_json` and its tagged-object transport to the loader or shared host layer. Keep typed parameter binding and filter construction in core. Preserve the existing parameter syntax and error cases for CLI callers. For field paths, preserve the exact canonical display, the JSON escape rules for double-quoted segments and the distinct single-quote rules. The field-path parser can keep its general name and call the explicitly named JSON helper for the double-quoted case. Test quotes, backslashes, control characters, Unicode surrogate pairs and invalid escapes. Reuse single-token helpers where appropriate. Keep the borrowed fast path and reusable scratch for source scans. Moving conversions must not add a serialize-and-reparse step per value.

## 3. Tests and benchmark ownership

Remove the test-only loader source inclusions, crate aliases, registration shortcut and macro bridge in core. Move real document, schema, framing and host compatibility tests beside the corresponding loader or into the existing CLI integration suite. Keep private planner, posting, publication and resource assertions in core using typed records or a small neutral test reader. Preserve the assertion and the code path it exercises when relocating a test.

Move benchmark manifest I/O, encoded benchmark parameters, report filenames and both Musenalm manifest files out of `crates/core`. Update all example, script and test paths. Preserve manifest bytes, case IDs and expected results. `Query::from_workload` currently consumes a benchmark request type. Keep any reusable typed query construction in core and put the benchmark adapter with its callers. Keep core independent of the CLI and loaders in normal and test builds.

## 4. Acceptance and delivery

- Run `git ls-files crates/core | rg -i json` and `git grep -in json -- crates/core` to audit ownership. Expected matches include explicitly named JSON token helpers, their filenames, callers, tests and documentation. Review each remaining use for scope. Built-in format selection, document decoding, schema-file handling, loader source inclusions or disguised dependencies in core fail acceptance. Also inspect the helper implementations to catch JSON-specific behavior hidden behind generic names.
- Test a core-only consumer with typed records, numeric filters, bound parameters and escaped field names. Test an unrelated reader, both shipped loaders separately, and both installed together. Compare record IDs, configuration hashes, query results, original bytes and diagnostic behavior against the parent.
- Run `cargo fmt --all -- --check`, `cargo check --locked --workspace --all-targets`, `cargo test --locked --workspace` and the release `external_loaders` suite. Retain the gated multi-file JSONL publication regression. Verify the existing CLI, TUI and desktop configuration fixtures.
- Keep performance validation focused. Compare a representative many-document build, a single large JSONL build and a borrowed-scan query with counters. If variance suggests a regression, repeat the affected case. Report added allocations, parses or reads, and investigate repeatable slowdowns before completion.
- Append a completion report with the final SHA, API migration mapping, dependency and naming checks, test results and paired measurements. Commit it with the integration branch.

## Implementation report

Completed on 2026-11-25. The task is `format-independent-core` on
`integration/format-independent-core` at
`/home/simon/source/objekte-integration-format-independent-core`.
The user approved current local master
`cc2c11790832af084566f7f35eed459af557c5f0` as the base. The final
implementation revision is `2f3cef6a60aa518e5f6fe9dffb040b3d6a5226cb`.

- `3602ede09a` made source selection opaque and moved default folder identities
  into the loader.
- `2e77f90e48` moved JSON value and parameter transport to the JSON loader and
  Musenalm workload I/O to the CLI.
- `da6b4740c4` replaced the core loader inclusions with neutral fixtures and
  relocated format-specific tests.
- `2f3cef6a60` enabled borrowed-scan test counters only for the loader's scanner
  tests. The benchmark build does not enable that instrumentation.

### API and ownership

| Previous API | Replacement |
| --- | --- |
| `SourceRegistry::builtin()` | Call `objekte_loader_json::register`, `objekte_loader_jsonl::register`, or `objekte_cli::loader::installed_sources` |
| `SourceFormat::Json` | `objekte_loader_json::JSON_DOCUMENTS` or `objekte_loader_json::folder` |
| `SourceFormat::JsonDecodedScalars` | `objekte_loader_json::JSON_DECODED_SCALARS` or `objekte_loader_json::decoded_scalar_folder` |
| `SourceFormat::JsonLines` | `objekte_loader_jsonl::JSON_LINES` |
| `SourceFormat::Registered(id)` | `SourceFormat::registered(id)` |
| `CollectionDefinition::folder` for a specific identity | `CollectionDefinition::folder_with_identity` or a loader folder helper |
| `Value::from_json` | `objekte_loader_json::value_from_json` |
| `parse_parameter_json` in core | `objekte_loader_json::parameters::parse_parameter_json` |
| `Query::from_workload` | `objekte_cli::workload::query_from_workload`. Typed callers can use `Query::from_syntax` and `Query::with_parameters`. |

`git ls-files crates/core | rg -i json` reports the JSON string token module
and its test file. `git grep -in json -- crates/core` reports those names and
callers plus the existing single-number grammar and resource-limit messages.
The core normal, build, and test dependency graph has no `serde_json`, CLI, or
loader crate. Its production and test source files use no loader inclusions.
The manifests moved byte-for-byte to `crates/cli/benches`.

| Manifest | SHA-256 |
| --- | --- |
| `musenalm_queries.json` | `56f2ba99e3a889b95975a6b460c4ef894732ad769f99e6f69086a3f0d3ac91c0` |
| `musenalm_queries_extended.json` | `58677b830a55027ba1d2b6dfec9189f0685d47e2e41a8e20639ac1afb94cc94f` |

### Validation

- `cargo fmt --all -- --check` and
  `cargo check --locked --workspace --all-targets` passed. The workspace check
  emitted no compiler warnings.
- `cargo test --locked --workspace` passed with 1,079 tests and doctests. The
  existing manual and benchmark tests accounted for 20 ignored cases.
  CLI, TUI, desktop configuration, explicit schema, cancellation, and worker
  draining fixtures passed.
- `cargo test --locked --release -p objekte-cli --no-default-features --test external_loaders`
  passed with 21 tests and one manual benchmark ignored. The parent suite
  passed with six tests. Both revisions passed the gated peer-publication and
  multi-file one-pass locator tests.
- `cargo run --locked -p objekte-search-core --example direct_records` selected
  `example-1` using a typed numeric parameter, an escaped field name, and a quoted
  query literal.
- The parent captured both legacy and generic selectors using the repository and
  programmatic fingerprint functions. The candidate matched all four captured
  hashes, the four existing fingerprint fixtures, and the prepared-folder
  fingerprint. The parent document-reader tests and their relocated
  candidate counterparts passed with the same query, value, and diagnostic
  assertions. The release host suite checked original bytes, generated and
  relative-path IDs, malformed-record handling, and independent and combined
  registration. A custom reader registered as `json` remained governed by its
  own declared capabilities.
- Private optional planner probes now print typed debug output. Archived
  reproduction scripts and their captured JSON remain tied to the revisions
  that produced those captures.

### Paired release measurements

Both revisions used an Intel Core i9-13900K with 32 logical CPUs, x86-64 Linux,
Rust 1.100.0, and the locked release build of the CLI examples with
`--no-default-features --features detailed-profiling`. Alternating pairs used
the same Musenalm root and the same generated 100,000-record JSONL file.

- `bench_build_musenalm --root <musenalm> --build-workers 32`
- `bench_jsonl_file_concurrency <fixture> one 32`
- `bench_musenalm --root <musenalm> --profiles ascii-off --query-workers 1 --cases Q030 --repetitions 7 --result-dir <run>`

Eight alternating pairs used all 32 logical CPUs with the performance
governor. The query column is the median of seven samples per invocation.
The RSS columns are process high-water marks and include startup.

| Workload | Workers | Parent median | Candidate median | Change | Parent median peak RSS | Candidate median peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Musenalm build, 148,486 documents | 32 | 684.801 ms | 705.520 ms | +3.0% | 4,273,600 KiB | 4,258,110 KiB |
| Single JSONL build, eight mixed pairs | 32 | 65.472 ms | 69.366 ms | +5.9% | 64,970 KiB | 65,024 KiB |
| Musenalm borrowed scan Q030 | 1 | 9.006 ms | 9.305 ms | +3.3% | 670,556 KiB | 667,238 KiB |

| Pair | Musenalm build parent → candidate | JSONL build parent → candidate | Q030 parent → candidate |
| ---: | ---: | ---: | ---: |
| 1 | 696.158 → 699.305 ms | 63.208 → 69.304 ms | 8.816 → 9.409 ms |
| 2 | 687.672 → 698.906 ms | 65.949 → 70.777 ms | 8.878 → 9.327 ms |
| 3 | 675.801 → 694.807 ms | 70.437 → 66.656 ms | 9.518 → 9.438 ms |
| 4 | 712.122 → 695.515 ms | 64.995 → 63.351 ms | 9.056 → 9.068 ms |
| 5 | 677.000 → 711.735 ms | 64.188 → 69.822 ms | 9.292 → 9.427 ms |
| 6 | 643.871 → 722.993 ms | 64.181 → 69.428 ms | 9.364 → 9.282 ms |
| 7 | 742.995 → 732.195 ms | 71.415 → 82.684 ms | 8.956 → 9.069 ms |
| 8 | 681.929 → 737.737 ms | 67.383 → 67.512 ms | 8.910 → 9.039 ms |

In 12 follow-up pairs restricted to single-file JSONL, the median was
64.586 ms on the parent and 63.375 ms on the candidate. The mixed-run
slowdown did not reproduce in isolation. Both revisions opened the file
once, read 28,800,000 bytes, and had one active file. Peak decoder jobs
ranged from 15 to 18 on the parent and 16 to 19 on the candidate across the
eight mixed pairs.

Both Musenalm builds performed 148,486 parses and 148,486 content opens,
read 193,297,230 bytes, and had zero failed parses. Every Q030 sample
matched 1,264 records and returned 50 hits. Each sample opened 2,251 files,
read 5,640,528 bytes, and completed with zero owned record parses. The result
digest was `860f6ba61d5e59d933c760529a0c9699a45b720e18d83119e448047c9a7fb104`
on both revisions. The benchmark has no process-wide allocation counter.
The relocated scanner tests verify scratch reuse and exercise the combined
path without constructing owned text segments. JSON numeric conversion
uses the borrowed numeric token on its fallback path, and benchmark
parameter conversion consumes the decoded value directly.

The integration branch and worktree are retained for review. No subordinate
worktrees were created.
