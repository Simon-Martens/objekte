# Objekte JSON and JSONL loader libraries

## Assignment 3 · External file format backends

Deliver two separately consumable Rust libraries, one for JSON documents and one for JSON Lines. An application can register either, both, or neither. The JSON library accepts an optional JSON Schema file and translates its supported declarations into Objekte's typed collection schema. An application that depends only on `objekte-search-core` can still build and query an index of directly supplied records.

**Starting point.** Complete the [core-managed file ingestion assignment](https://chatgpt.com/library?file_id=libfile_17265df3c61c8191ab96b1e3f0968be2&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2) first. Create `integration/external-json-loaders` at the completed and pushed tip of `integration/core-managed-file-ingestion`. Use the sibling worktree `/home/simon/source/objekte-integration-external-json-loaders`. Record the actual parent SHA and use it for comparisons. The source map below reflects the earlier ingestion branch. Follow moved code on the actual parent. Follow `AGENTS.md` and `docs/ai/README.md` for commits, pushes and the completion report. This assignment requires workspace library crates and integration into existing applications. Registry publication and a merge to master are separate actions.

## 1. Package ownership

| Package | Responsibilities |
|---|---|
| `objekte-search-core` in `crates/core` | Public loader contract, typed values and programmatic schema, schema validation and conversion, source registration, optional file catalog, worker and memory budgets, record identity and slots, segment construction, indexes, query semantics, liveness and publication. |
| `objekte-loader-json` in `crates/loader-json` | Single-document JSON reading, normalization to typed values, existing JSON text representations, projected and original-byte reads, JSON-specific scan acceleration, optional JSON Schema file loading and compilation. |
| `objekte-loader-jsonl` in `crates/loader-jsonl` | JSON Lines framing, bounded streaming, scheduling independently decodable chunks through the shared worker budget, file-relative locations and line diagnostics. Reuse the JSON crate's record decoder and projection routines through a narrow library interface. |
| CLI, TUI, desktop and other file-backed hosts | Select and register the installed loaders, resolve configuration, pass compiled collection declarations to core, and preserve existing user-facing defaults and flags. Use the same configuration path across frontends. |

The JSON library depends on core. The JSONL library may depend on the JSON library for decoding. Registering JSONL alone installs only its own source registration. Core's normal dependency graph contains neither loader, including with its default features. Keep command-line code out of both libraries. Preserve the existing stable format IDs, extensions and representation versions for unchanged behavior, including `json`, `json_decoded_scalars` and `jsonl`. An unknown or unregistered format produces a configuration error before indexing.

## 2. Public boundary for reading source files

Retain the two ingestion routes from assignment 2. Direct typed records and versioned direct providers continue to work with no file service. For file-backed records, the catalog opens the selected file, observes metadata on the handle, and coordinates freshness and index publication. **The registered backend performs source-content I/O.** Give it an input tied to the opened file. A bare path or core-prefilled whole-file byte buffer is insufficient as the primary backend contract.

| Operation | Backend | Core |
|---|---|---|
| Import and replacement | Read the supplied input and emit typed records, intrinsic IDs when applicable, searchable text or its coverage declaration, original byte spans, and diagnostics. An incremental emitter can hand off completed records or bounded batches. | Validate the emitted ranges and configured identity policy, apply the collection schema, assign slots, build segments and publish each complete file revision atomically. |
| Logical, projected or original-byte reread | Retrieve the requested record or range from the supplied revision-bound input. Preserve projections, original bytes, raw fallback and the backend's query-time text representation. | Choose the projection, validate the expected source, apply query semantics and return the appropriate payload or stale/unavailable outcome. |
| Content verification | Compute any required fingerprint or comparison on the same input, normally during the existing read. Shared reader utilities can implement this outside core. | Own the verification policy, expected fingerprint or file identity, revision checks, retry, and final decision to publish or serve a result. |

Apply the boundary to initial and progressive builds, add/update, reconciliation, content-preserving moves that need verification, compaction paths that actually reread content, projected query verification, relations and source preview. Core can pass through backend-returned bytes and index supplied logical values and text. Its metadata-only queries, exact indexed counts and direct-record operations must require zero source-content reads. Preserve assignment 2's same-handle observations, equal-size/equal-mtime replacement test and pinned-view behavior. Index storage, ingestion spools and application configuration have their own I/O contracts.

Keep efficient revision-bound read sessions and grouped range reads where they already exist. Dispatch once per file, batch or scan session where practical. A trait object per scalar value, a fresh file open per candidate, an additional full-file hash per record, or reparsing every projected field would defeat the existing search design.

## 3. JSON documents and optional JSON Schema

1. **One document remains one record.** Preserve supported top-level values, including arrays and scalars, exact numeric behavior, escaping, Unicode, raw byte ranges, malformed-input diagnostics and fallback. Preserve the two existing JSON text representations separately. A field predicate, FTS query or evidence request must see the same strings and boundaries as on the parent branch.
2. **Expose an explicit optional schema-file input through the JSON library.** A host must be able to configure a JSON collection with a schema path, or configure it without one, using public library APIs. Prepare the schema once per collection configuration before launching file workers. Return the typed schema, sort declarations and the configuration identity inputs the host needs. The host can pass these directly into the core builder. No schema file is needed for undeclared JSON, opt-in inference, a manually constructed core schema or direct ingestion.
3. **Move the file format and its dialect into the JSON library.** Migrate `schema/json` and the JSON-specific parts of `repository/configuration/loader.rs`. Preserve the supported JSON Schema subset and Objekte annotations as implemented, including local references, nullability, required fields, nested arrays/objects, numeric and temporal declarations, explicit index inclusion/exclusion, sort declarations and relations. Retain existing rejection or diagnostic behavior for malformed declarations, unresolved/cyclic references and unsupported external references. This is an ownership change to the existing dialect.
4. **Keep core's schema authoritative over typed records.** The JSON compiler translates file syntax into the existing `Schema`, field, sort and relation declarations. Core continues to apply typed validation, conversion, inference, indexing and query binding for every source in the collection. Apply conversions once on the appropriate data path. Sharing a collection with directly submitted records must produce the same typed interpretation. JSONL can use the same compiled schema for each line through the existing collection configuration. Maintain a single compiler.
5. **Preserve explicit and repository schema modes.** Explicit mode loads exactly the requested file with the parent's path resolution, treatment of `x-objekte` and unavailable relations. Repository mode still reads `.objekte/collections/*.schema.json`, resolves collection names before cross-collection relations, honors source selection, identity and locale, and supplies protected/excluded schema paths to the host. JSON syntax and annotations are interpreted outside core. Generic catalog containment and discovery rules stay in core. An invalid schema fails setup with the file path and existing field-level diagnostic. A schema file inside the data directory must not become an indexed record.
6. **Keep configuration fingerprints meaningful.** Where the parent fingerprints exact schema bytes, the JSON library fingerprints those same bytes and hands core an opaque digest or equivalent neutral identity. Preserve distinct collection/reader/representation identities, including two collections using the same loader with different schemas. No repeated schema-file reads per data file, record or query. Exercise compatibility with an existing supported saved index or state, and its existing invalidation behavior after a schema change.

## 4. JSONL and parallelism

The callback is per file. Objekte controls file scheduling and may invoke the same registered backend concurrently for several files. The backend cannot rely on file discovery, callback start, completion or publication order. Keep parsing state local to each invocation. An attempt may be retried. Keep emitted data scoped to that attempt.

**Inside a large JSONL file, the JSONL library owns the split into independently decodable work.** It reads forward, identifies complete record boundaries, and hands bounded chunks to the existing shared executor or an equivalent scoped scheduling facility supplied by core. Backend code decodes the lines and emits records. Core accepts the results and decides segment sizes, placement, number and publication. A caller using the JSONL library supplies neither worker threads nor segment definitions. Byte ranges returned with already-decoded records are locators for subsequent access.

- Preserve the existing framing contract. Ignore blank and whitespace-only lines as on the parent, exclude the LF delimiter, preserve the prior CR handling, handle a final record without LF, and keep absolute byte offsets and diagnostic line numbers correct across input buffers. Preserve valid neighbours and established raw fallback behavior around malformed lines. Follow the parent's behavior for oversized records.
- Bound queued and in-flight chunks by both bytes and worker availability. Reuse input buffers where their lifetime permits. Capture fingerprints during the existing input pass if required. Support one worker inline, many small files concurrently, and multiple decode jobs for a single large file. Reuse one pool across files and intra-file jobs. The scheduler must make progress even when every worker has entered a different file callback. Avoid blocking all workers waiting for nested work queued to the same pool.
- Completion of chunks may be out of order. Retain byte position, source ordinal and attempt identity independently of worker timing. Preserve per-file record ordering wherever the original format/identity contract requires it, using bounded reordering if needed. On errors and cancellation, release outstanding work and reservations and preserve file-atomic publication. Whole-document JSON can decode one document serially while independent files parse in parallel.

## 5. Remove hidden built-in assumptions

Before moving files, trace the following paths on the completed parent. Port tests beside their new owners. Keep at least one integration test that consumes the loaders entirely through public APIs.

| Earlier implementation | Required change |
|---|---|
| `source/json/{document,scalars}`, `source/json_lines.rs`, `source/framing.rs`, `construction/file_preparation.rs` | Move format parsing, JSONL framing and format-specific scheduling decisions out of core. Keep execution capacity and segment builders generic. Reuse the same JSON decoder for ordinary ingestion and JSONL. |
| `source/access`, `source/extraction.rs`, `lifecycle/mutation` | Route source-content reads, grouped rereads, byte comparisons and content hashes through the backend or its input utilities. Keep filesystem metadata and file-version validation in the catalog. |
| `source/registry.rs`, `SourceFormat`, configuration identity and builder defaults | Remove unconditional built-in registration and format-name switches from core execution paths. Represent capabilities such as one record per file, supported identity policies, text coverage and trusted projected extraction explicitly. A registration using a familiar name does not acquire capabilities by spelling. |
| `source/verification.rs`, `query_predicate/scan.rs`, query runner source/segment scans, relation execution | Preserve borrowed scalar scans, combined projected extraction plus FTS, match evidence and early exits through neutral optional interfaces. Core owns query truth and fallback. Backends can implement an optimized operation or decline it. Do not expose private planner structures wholesale to make the crates compile. |
| `model/json.rs`, `adapter/helpers/json_string.rs`, application setup, core examples and benchmarks | Put source-document conversion and JSON unescaping with their consumer. Preserve exact-number and query-literal semantics. Keep independent uses of JSON for query parameters, diagnostics or benchmark output where appropriate. Exercise the dependency boundary with normal Cargo dependency edges. Move loader-dependent fixtures/examples to a host or integration-test location if a reverse dependency would otherwise be needed. |

Configure JSON defaults once in shared host setup so the CLI, TUI and desktop continue to open normal JSON folders and configured repositories. Preserve `--schema`, opt-in auto-index, file mutations and format-specific payload displays. Preserve host behavior for an installed empty catalog and an index with mixed file-backed and direct records. Update the public Rust examples and document how existing callers of the built-in registry and core JSON schema loader migrate.

## 6. Acceptance and evidence

| Case | Evidence required |
|---|---|
| Independent consumers | Build core alone and run a direct-only query without either loader. Build and run minimal consumers registering JSON only, JSONL only, and both together. Show the normal Cargo dependency trees. A JSONL-only registry must reject an unregistered JSON source even if the decoder code is linked. Exercise an unrelated custom reader with the public contract. |
| Optional schema | Run the same JSON fixture with no schema, an explicit JSON Schema file and equivalent programmatic typed declarations. Verify declared types, index admission/opt-outs, FTS, field filters, sort and relation behavior. Check invalid input diagnostics, missing and cyclic references, excluded schema paths, canonical schema fingerprint, and a second collection with a different schema. Count schema opens/compilations during ingestion and repeated queries. |
| Source and query parity | Compare parent and candidate on `json`, `json_decoded_scalars` and `jsonl`. Check IDs, logical values, raw payload bytes, ranges, status/diagnostics, result membership, sorted order and evidence. Include escaped strings, multibyte text, nested values, exact numbers, malformed records, raw fallback, projected residual queries and a relation target. A same-name custom reader that lacks a capability must take the generic path. |
| Mixed ownership and stale input | In one collection, ingest a file and a direct record. Update and replace the file, including a same-size/same-mtime rename over its path. Verify the previous pinned view, current catalog, reverse lookups, `@file.*` absence on the direct record, and safe original-byte retrieval. Repeat after compaction or reconciliation using the established assignment 2 regression. |
| Concurrency and bounded work | Use instrumented callbacks and barriers to demonstrate overlapping file callbacks on one backend instance and overlapping decode jobs on one large JSONL file with multiple workers. Re-run with one worker and with several large files saturating the pool. Verify equivalent record identities and ranges, a single successful file publication per revision, bounded input storage, cancellation cleanup and one normal content pass. Chunk completion order must not change record-to-byte associations. |
| User entry points | Run CLI folder and explicit-schema searches, CLI/TUI repository configuration, and the desktop's ordinary open/query path or its established integration fixture. Supply a runnable library example showing a JSON schema path and another showing parallel JSONL without manual segmentation or thread management. |

Use the completed assignment 2 parent and the final implementation commit on the same host, datasets, release profile and worker settings. Cover many small JSON documents, one JSONL file of at least 100,000 records, and multiple files in parallel. Run with one worker and the established high-throughput configuration. Include representative exact metadata/count, projected residual, decoded-scalar FTS and original-byte queries. Report build time, records/s, peak RSS, bytes read, chunk concurrency and a query-work counter that would expose a lost fast path. Resolve repeatable material regressions and demonstrated redundant work. Microsecond differences in isolation carry no release gate.

Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, `cargo test --workspace`, the standalone consumer examples, and the targeted release tests and measurements. Append the implementation report with the parent and final code SHAs, actual dependency graph, source-ownership inventory, commands, comparison results and outstanding limitations. Push the completed assignment and report on the third integration branch.

## Implementation report

Completed 2026-11-25 on `integration/external-json-loaders` in `/home/simon/source/objekte-integration-external-json-loaders`. The requested parent and benchmark baseline was `a9ad979d0b9229b48599be479a459055ab8a554a`. The previous master tip was `d8d703dbd2`. After fetching and verifying ancestry, master was fast-forwarded and pushed to the exact requested parent. Assignment 3 was pushed on the integration branch.

### Implementation

| Commit | Scope |
| --- | --- |
| `b5e2909a48758e80219891c2e4b500dfe3dd500e` | JSON Schema compiler, repository dialect, exact-byte schema digest and host configuration |
| `4f2b402f6f05ca0cf8e8d99da6861f8f276ee5bf` | External readers, shared decoder, source-content I/O, borrowed scans and bounded framing |
| `740512d84169dc326304efc4629909eb76b6c71f` | Consumer tests, registration documentation and runnable examples |

Final implementation revision `740512d84169dc326304efc4629909eb76b6c71f`. No subordinate worktrees were created.

- `crates/loader-json/src/document` owns JSON document parsing and projection, both JSON text representations, borrowed scalar scanning and unescaping. `crates/loader-json/src/{schema,configuration}` owns the existing schema dialect, explicit schema selection and configured-repository interpretation. The compiled typed declarations and digest are passed to core. The Musenalm benchmark retains its historical fixture adapter so the two binaries exercise identical workloads.
- `crates/loader-jsonl/src/{reader,framing}` owns LF framing, file-relative record locations and bounding of retained line state. JSONL calls the JSON record decoder and projection API. Registration installs only `jsonl`.
- `crates/core/src/source` owns registration and the format-independent document, range, fingerprint, framing and scalar verification contracts. Core still owns catalog metadata and revision checks, same-handle freshness, coalescing adjacent ranges, worker admission, attempt isolation, typed schema application, record identity, segmentation, publication and query semantics. The input adapters can fingerprint during the existing read. File reads in builds, mutation, projected verification, relation targets and payload delivery use the registered reader. Direct records and exact indexed locator/count queries avoid reading sources.
- CLI, TUI and desktop select the same loader registrations. Host integration tests and benchmarks moved to `crates/cli`. Loader format tests live beside their implementations. Core's white-box tests include those sources only in test builds, so they can probe internal query and publication counters without a reverse Cargo dependency.
- A caller migrates by installing the desired readers in an empty registry and passing it to the existing build entry point. The [Rust loader guide](../../development/external-loaders.md) documents registration, optional schema compilation and examples.

### Validation

- `cargo fmt --all -- --check` and `cargo check --workspace --all-targets` passed. `cargo test --workspace` passed 1,085 tests across 29 suites with 21 ignored.
- `cargo test --release -p objekte-cli --no-default-features --test external_loaders` passed all five tests. The cases cover JSON-only and JSONL-only registration, both loaders, a same-name custom reader doing opened-handle I/O, exact counts performing zero content reads, and optional schemas equivalent to direct declarations. Removing the schema after compilation left ingestion and repeated queries working. Editing only schema whitespace changed the configuration fingerprint in a two-collection setup with different declarations.
- That release suite also built five large JSONL files using one and four workers. Both runs read exactly five files once, published each revision once, and retrieved the same record-to-byte locators. The workspace suite exercised overlapping file callbacks and large registered decoders, late worker failure and cancellation drain, zero-record chunks, published segment identity, stale same-size and same-mtime replacements, mixed ownership, compaction and reconciliation.
- Standalone release examples `direct_records`, `json_collection` with and without a schema, and `parallel_jsonl` all ran successfully. `cargo tree -e normal -p` showed the workspace package edges below. Core still uses serde JSON for logical/programmatic values and query literals.

  ```text
  objekte-search-core
  objekte-loader-json → objekte-search-core
  objekte-loader-jsonl → objekte-search-core, objekte-loader-json
  ```

- Release `obj` found the expected record in folder mode, with `--schema`, and with `--repo`. The explicit schema inside the data folder was excluded. A TUI pseudo-terminal smoke test ran `obj --repo <fixture> --tui --fts Goethe`, drew the configured collection and query, and exited successfully. The desktop backend's established folder open, progressive catalog, query and pinned view fixtures passed in the workspace suite.
- A release consumer ran the same small fixture at both revisions with `json`, `json_decoded_scalars`, and `jsonl`. Its output included IDs, logical values, original bytes and ranges, file status, malformed-record diagnostics, sorted order, query evidence, and work counters. The fixture used an escaped string, multibyte text, nested objects and arrays, exact numbers, whitespace and CRLF, a missing final LF and a malformed record. The outputs were byte-identical with SHA-256 `44b001cade1d6e5859170a5c1b26d1c8fe673a421946c31b695622fff8cb3569`. The established relation fixtures covered relation target verification and projected residuals.
- The exact parent already used a live, in-memory index. Compatibility evidence covers its fixed configuration-hash fixture, the parent/candidate configuration identity, stable representation versions and schema-change invalidation. A disk snapshot round trip would require a persistence entry point outside the supported parent.

### Construction benchmarks

Linux, Intel Core i9-13900K, 32 logical CPUs, Rust 1.100.0. Separate release targets were built at the exact baseline and final implementation revisions using detailed profiling. The baseline benchmarks were built with `-p objekte-search-core --features detailed-profiling`; the candidate used `-p objekte-cli --no-default-features --features detailed-profiling`. Paired fresh processes shared the same host and warm filesystem data. The reported medians are from three samples per revision and worker setting.

`bench_build_musenalm --root /home/simon/source/objekte/examples/musenalm --build-workers <1|32>` used the configured 148,486-document corpus. `bench_managed_file_scale <temporary-root> <jsonl|registered-mixed|registered-many> <1|32>` generated a 100,000-record JSONL file, a 100,000-record registered file with a small neighbor, and 8,192 small registered files, respectively. Throughput is live records divided by elapsed build wall time. Peak RSS comes from Linux `VmHWM`.
| Workload | Workers | Median wall ms, parent → candidate | Records/s, parent → candidate | Peak RSS MiB, parent → candidate | Source bytes | Peak decode/file jobs, parent → candidate |
|---|---:|---:|---:|---:|---:|---:|
| Musenalm JSON | 1 | 3457.97 → 3451.07 | 42,940 → 43,026 | 503.5 → 503.3 | 193,297,230 | 1 → 1 |
| Musenalm JSON | 32 | 724.28 → 701.07 | 205,013 → 211,799 | 664.7 → 654.5 | 193,297,230 | 32 → 32 |
| One JSONL file | 1 | 255.09 → 256.02 | 392,020 → 390,600 | 54.9 → 54.9 | 30,000,000 | 1 → 1 |
| One JSONL file | 32 | 57.80 → 57.03 | 1,730,191 → 1,753,417 | 63.4 → 62.8 | 30,000,000 | 16 → 16 |
| Registered large and small | 1 | 271.32 → 267.00 | 368,568 → 374,532 | 55.4 → 55.5 | 30,000,023 | 1 → 1 |
| Registered large and small | 32 | 130.94 → 134.35 | 763,711 → 744,339 | 59.1 → 59.1 | 30,000,023 | 6 → 6 |
| Many registered files | 1 | 193.79 → 191.72 | 42,273 → 42,728 | 25.8 → 25.7 | 188,416 | 1 → 1 |
| Many registered files | 32 | 300.32 → 297.77 | 27,278 → 27,511 | 31.4 → 31.9 | 188,416 | 4 → 4 |

The configured Musenalm index signature was identical in every sample (`ec81c48ab00729ceaf537d5505ca2d72a2067a8104dd86554a0862d595148dbe`). The JSONL case created 391 framing chunks and 25 segments, read the 30,000,000-byte file once, and published one file revision. At 32 workers, both revisions had a median peak of 16 decoder jobs, 5,185,836 bytes of pending and admitted input, and 89,388 bytes of pending input. The large registered file shared the pool with the small neighbor, with 392 framing chunks and two publications. Both many-file runs admitted work from up to four registered files concurrently at the high worker setting.

### Query benchmarks

`bench_musenalm --root /home/simon/source/objekte/examples/musenalm --profiles ascii-off --query-workers <1|32> --cases Q001,Q030,Q034,Q035,Q054,Q081,Q109,Q116,Q136,Q137,Q144 --repetitions 7` ran at both revisions. The result digests and work counters agreed at both worker settings. The bytes column is the general verification counter. Q137 separately recorded 50 selected original-byte payload reads totaling 54,704 bytes with zero record parses on each revision. Q109 and Q116 exercised exact metadata and count without source reads. Q030 retained its borrowed FTS route with zero owned record parses. Q081 exercised a projected residual, and Q144 combined FTS with a structured restriction.

| Query | Workers | Median ms, parent → candidate | Matches | Bytes read | Parses |
|---|---:|---:|---:|---:|---:|
| Q001 | 1 | 39.581 → 38.635 | 3,269 | 12,568,541 | 4,872 |
| Q030 | 1 | 8.474 → 8.810 | 1,264 | 5,640,528 | 0 |
| Q034 | 1 | 0.004 → 0.004 | 1 | 0 | 0 |
| Q035 | 1 | 0.022 → 0.022 | 124 | 0 | 0 |
| Q054 | 1 | 1.528 → 1.515 | 66 | 0 | 0 |
| Q081 | 1 | 3.362 → 3.350 | 888 | 2,405,741 | 888 |
| Q109 | 1 | 0.186 → 0.192 | 2,651 | 0 | 0 |
| Q116 | 1 | 0.193 → 0.194 | 70,428 | 0 | 0 |
| Q136 | 1 | 2.625 → 2.782 | 133,948 | 0 | 0 |
| Q137 | 1 | 0.178 → 0.179 | 81 | 0 | 0 |
| Q144 | 1 | 65.522 → 65.157 | 3,939 | 10,102,463 | 8,570 |
| Q001 | 32 | 5.818 → 5.838 | 3,269 | 12,568,541 | 4,872 |
| Q030 | 32 | 1.722 → 1.676 | 1,264 | 5,640,528 | 0 |
| Q034 | 32 | 0.005 → 0.004 | 1 | 0 | 0 |
| Q035 | 32 | 0.023 → 0.023 | 124 | 0 | 0 |
| Q054 | 32 | 1.621 → 1.595 | 66 | 0 | 0 |
| Q081 | 32 | 0.767 → 0.904 | 888 | 2,405,741 | 888 |
| Q109 | 32 | 0.316 → 0.208 | 2,651 | 0 | 0 |
| Q116 | 32 | 0.205 → 0.205 | 70,428 | 0 | 0 |
| Q136 | 32 | 2.612 → 2.857 | 133,948 | 0 | 0 |
| Q137 | 32 | 0.178 → 0.177 | 81 | 0 | 0 |
| Q144 | 32 | 11.336 → 12.474 | 3,939 | 10,102,463 | 8,570 |

The first 32-worker run put Q081, Q136 and Q144 about 18%, 9% and 10% higher, respectively. Three additional alternating pairs with 15 samples per case gave medians of **0.651 → 0.675 ms**, **2.733 → 2.805 ms**, and **11.210 → 11.605 ms**, respectively. The Q144 per-process medians ranged from 10.835 to 11.687 ms on the parent and 11.590 to 12.132 ms on the candidate. The repeat measurements and identical work counters showed no material regression. The largest construction increase was 3.41 ms (2.6%) on the 32-worker large-plus-small registered workload.

A standalone release consumer also registered `json_decoded_scalars` for the Musenalm `data/entries` directory, disabled the verified cache, and searched `Taschenbuch` with evidence. The residual case added `preferred_title ~ 'Taschen'`. Each process warmed the query once and then ran 11 measured samples. This table gives the median of three process medians at each setting. Each case read 2,251 files and 5,640,528 bytes and recorded 2,251 parses at both revisions.

| Decoded-scalar query | Workers | Median ms, parent → candidate | Matches |
|---|---:|---:|---:|
| Literal FTS | 1 | 9.371 → 9.395 | 2,057 |
| Literal FTS and projected residual | 1 | 11.546 → 11.365 | 1,305 |
| Literal FTS with original bytes | 1 | 9.388 → 9.497 | 2,057 |
| Literal FTS | 32 | 1.631 → 1.618 | 2,057 |
| Literal FTS and projected residual | 32 | 1.681 → 1.751 | 1,305 |
| Literal FTS with original bytes | 32 | 1.672 → 1.719 | 2,057 |

Local raw benchmark outputs and the paired-run scripts were recorded under `/tmp/objekte-a3-evidence`. The integration worktree is retained for review.
