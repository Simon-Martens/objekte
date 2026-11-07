# Objekte `obj` command and general JSON support: implementation plan

## 1. Assignment and execution basis

Implement a general-purpose command named **`obj`**. It opens one directory as one JSON collection, constructs or restores the shared bigram index, executes structured queries or full-text searches, and returns file metadata, query measurements, and the complete JSON document for each match.

The command must work without a schema, field declarations, an application-specific repository configuration, or knowledge of the document field names. One `.json` file is one record, including a file whose root is an array or scalar. A root array must not be split into separate records.

This document is an implementation work order for a supervisor, implementation agents, and bounded review/test subagents. Preparing this document does not start implementation or authorize changes to active agents' worktrees.

### 1.1 Assume the preceding work is completed

Treat the preceding optional-indexing, broader-sorting, and streaming assignment as the prerequisite implementation. In particular, assume these capabilities are available at the integration baseline:

- Conservative bigram restrictions for undeclared text FILTER predicates, including the existing empty-selection rules.
- Explicitly opt-in inference and automatic typed postings.
- Sorting on undeclared fields, explicit array selectors and reducers, with bounded retained keys and deterministic ordering.
- Synchronous ordered streaming, cancellation, backpressure, and correct cache publication.
- The schema-free folder-definition helper and relative-file-path identity, if delivered through that assignment.

Do not implement competing versions of these features or restart from an older branch. Their agents may still be working when this plan is handed over. Independent CLI argument/output work may proceed against frozen interfaces, but library integration and acceptance must use their completed, reviewed integration result.

The last repository inspection used `development/local-file-search-core` at `f374101b4839f28199a99677118ac96869203bb0`. That commit is an architectural reference, **not** the required implementation base. The supervisor selects and records a newer fixed commit containing the prerequisite work.

### 1.2 Explicit additions to the preceding assignment

This assignment supersedes two limits in the preceding specification:

1. Its exclusion of arbitrary JSON roots and heterogeneous arrays is removed. Those shapes must now be first-class logical records in schema-free mode.
2. Its streaming API delivered one requested page. This assignment adds an explicit all-results execution scope, because the CLI must return every match by default without repeated pagination and rescanning.

Other established contracts remain in force. In particular, an explicitly declared type violation still produces the existing diagnosed fallback outcome. General JSON support must not silently disable declared validation.

## 2. Required behavior and scope

| Area | Required behavior |
| --- | --- |
| Executable | `obj`; package name `objekte-cli`; workspace member `crates/cli`, unless a current equivalent already exists. |
| Source root | Current working directory by default; `--path PATH` or `-p PATH` overrides it. |
| Collections | Exactly one collection, internally named `files`. No collection configuration file is read. |
| Discovery | Recursive by default; `--no-recurse` selects immediate children only. |
| Format | Built-in JSON adapter only. One complete JSON document per `.json` file. |
| Logical admission | Objects, arrays, strings, numbers, booleans, and null roots; mixed arrays and nested mixtures. |
| Schema | Empty schema, no implicit schema reads, inference disabled. |
| Indexes | Shared bigrams and existing inventory/identity structures; no automatically declared typed postings or sort columns. |
| Text corpus | Every decoded string value and every other scalar value's JSON token representation; property names and container punctuation excluded. |
| Queries | One structured query, one literal FTS query, or a line-based query file. |
| Results | All matching records by default; full original JSON content and metadata; explicit optional pagination. |
| Persistence | No disk index use unless requested; explicit write/load flags with directory arguments. |
| Delivery | Ordered NDJSON events on stdout; operational diagnostics on stderr. |
| Performance | One construction pass, one repository instance per invocation, one query execution per query, bounded result delivery. |

Do not add a TUI, REPL, daemon, filesystem watcher, SQL layer, automatic schema editing, application-specific Musenalm field registration, asynchronous runtime, automatic relation detection, or runtime source mutation. Do not add an external sort engine in the first implementation. Existing configured resource limits may reject an excessively large computed sort explicitly; silent truncation is prohibited.

The user prioritizes execution speed. Reuse the accepted library mechanisms and keep the CLI as an application boundary. Do not place query evaluation, inference, sorting comparators, or bigram algorithms in the CLI crate.

## 3. Command-line interface

### 3.1 Invocation

```text
obj [OPTIONS] [QUERY]
```

| Argument | Meaning |
| --- | --- |
| `QUERY` | One complete query expression. A complete double-quoted JSON string literal means literal FTS; otherwise it is a structured query. |
| `-p, --path PATH` | Collection directory; default is the process working directory. |
| `--no-recurse` | Disable descent into subdirectories. |
| `--fts TEXT` | Literal FTS without the extra JSON-string quoting needed by the positional form. |
| `--query-file FILE` | Read one query per physical line; `-` means stdin. |
| `--write-index[=DIR]` | Permit saving a snapshot; absent value means `<root>/.objekte`. |
| `--load-index[=DIR]` | Attempt snapshot restore; absent value means `<root>/.objekte`. |
| `--limit N` | Return at most N matches; omission means all matches. Zero is valid. |
| `--offset N` | Skip N matches in final result order; default zero. |
| `--count` | Request an exact total even if a finite limit permits earlier termination. All-results execution naturally learns the total. |
| `--index-alphabet ascii\|unicode` | Use the existing alphabet implementations; default remains the accepted core default, currently ASCII. |
| `--help`, `--version` | Normal CLI information; no discovery or index work. |

Use one established argument parser already used by the workspace if suitable. Do not build a custom general argument parser.

`QUERY`, `--fts`, and `--query-file` are mutually exclusive input modes. Multiple queries belong in `--query-file`; repeated positional arguments must not be silently joined. A query beginning with `-` can follow the normal `--` option terminator.

Optional index-directory values require `=`. This prevents `--write-index 'title = "A"'` from consuming the query as an index path. State this in `--help` and cover it in argument tests. Explicit relative index-directory paths are relative to the invocation's working directory. The omitted directory is relative to the selected collection root.

If both index flags specify directories, their normalized destinations must agree. A mismatched pair is an argument error; index copying is not part of this command.

No query input is allowed only with at least one index flag: perform build/write or restore/validation and produce startup/terminal events. Bare `obj` on a terminal gives concise usage and a missing-query error, without building an index. There is no implicit interactive prompt. Stdin is read only through `--query-file -`.

Exit status is `0` when the requested operation succeeds, including zero matches; `1` when any query/input-stream/execution/output operation fails; `2` for invalid arguments or collection/snapshot startup failure; and `130` for Ctrl-C cancellation. The intentional broken-pipe exception is specified in section 9.3. A failed query in a batch keeps the final status nonzero even when later queries succeed.

### 3.2 Examples

```bash
obj --fts Goethe
obj '"Goethe"'
obj -p ./documents 'title ~ "Goethe"'
obj -p ./documents 'year >= 1800 && year < 1900'
obj --no-recurse --query-file queries.txt
obj --write-index --fts Goethe
obj --write-index=/var/cache/obj/documents -p ./documents --query-file queries.txt
obj --load-index -p ./documents --fts Goethe
obj --load-index --write-index -p ./documents --query-file queries.txt
obj --limit 100 --count 'title ~ "Goethe" SORT BY title'
obj --write-index
```

Example `queries.txt`:

```text
"Goethe"
title ~ "Goethe"
year >= 1800 && year < 1900
@root[0] = "first"
@root ?= "text"
"Gr\u00fcnde"
```

The new `@root` syntax is specified in section 6. Compile examples against the final grammar during acceptance; do not ship examples supported only by a proposed parser.

### 3.3 Query-file contract

- Read UTF-8. Accept LF and CRLF, and an optional UTF-8 BOM at the start of the file.
- Trim surrounding whitespace; ignore blank lines. Do not invent a comment syntax for this version.
- A nonblank line whose first nonspace character is `"` must parse as exactly one complete JSON string literal, with no trailing tokens. Decode escapes once and execute literal FTS.
- Other lines go to the existing structured-query parser. Do not infer regex mode from `/.../`, reinterpret backslashes in structured expressions, or implement query grammar in the CLI.
- A malformed quoted string is an FTS-input error, not a reason to retry the line as a structured expression.
- `""` is an empty literal FTS query and follows the core's existing empty-needle semantics. A blank line is not a query.
- Newlines inside an FTS string use JSON escapes. There are no multiline query continuations.
- Preserve the original physical line number in every query event and error. Single direct queries have no file line number.
- Execute file queries sequentially against one repository. Repeated queries can reuse the core's legitimate in-process caches; do not rebuild or reopen between lines.
- Parse each query before executing it. An erroneous query produces a terminal error for that query, then later lines continue. The process exits nonzero if any query failed.
- A query-file read/encoding failure is a fatal input error. A readable file containing no queries is an input error unless the invocation is also explicitly performing an index-only operation.

## 4. Collection construction and discovery

Use the prerequisite's general folder constructor. If it does not support the collection directory being the repository root, add that capability centrally instead of configuring a synthetic child directory. Canonicalize the selected root once. The inspected discovery implementation can represent the root with an empty relative path; do not pass an absolute collection source path or rely on `.` where contained-component validation rejects it.

Configure an empty schema, no declared relations, no custom field postings, no registered sort columns, and inference disabled. A nearby JSON Schema is ordinary input if it is a selected `.json` file; it is never executed as configuration. Do not enable inference because a query mentions an undeclared field.

Discovery policy for this general folder command:

- Select regular files with the adapter's `.json` extension. Document the initial case-sensitive extension rule; `.jsonl` is not included.
- Include hidden JSON files and descend into hidden directories, consistent with admitting the folder's JSON documents.
- Explicitly exclude `.git` directories and the default/selected snapshot directories. Do this in traversal so large metadata trees are not visited unnecessarily.
- Do not apply `.gitignore` as an implicit input filter.
- Skip symlinks by default, using the existing policy. A symlink passed as the top-level `--path` is resolved when the root is canonicalized; nested symlinks are not traversed.
- A permission or discovery I/O error fails collection opening with the path and cause. Do not silently report a complete collection after omitting unreadable directories.
- An empty directory is a valid collection and produces zero-hit query results.

Set the discovery options explicitly; do not change the default discovery policy for unrelated library callers merely to implement the CLI.

Use the prerequisite's relative-file-path identity for one-record-per-file readers. Include the extension and all relative components; do not use basename stems or require an `id` field. `a/001.json` and `b/001.json` are distinct. The same relative path has the same identity after an unrelated file is added. Preserve the established reversible representation for platform paths, including non-UTF-8 names where supported; lossy conversion must not create identity collisions.

If the prerequisite omitted path identity, implementing it is a required library task. Do not substitute generation-dependent identifiers for the documented stable file identity. Keep internal slots/generations separate from public record identity.

## 5. General JSON logical records

### 5.1 Admission contract

In schema-free mode, every successfully parsed JSON document is a logical record, regardless of root shape or element-type variation:

```json
{"title":"A","values":["text",123,true,null,{"name":"B"},[1,"C"]]}
```

```json
["text",123,true,null,{"name":"B"},[1,"C"]]
```

```json
"a root string"
```

```json
null
```

Also cover number, boolean, empty-array, and empty-object roots. A null root is a present JSON value, not a missing document or unavailable logical record.

Do not wrap the document in a synthetic `value` object. Wrapping changes field paths, can collide with real fields, and changes the returned payload. Do not flatten arrays into separate records or discard members that do not match their neighbors' types.

### 5.2 Separate representation from validation

The current `ArrayValue::new` enforces homogeneous element types, and both the JSON reader and schema coercion require object roots. Those assumptions must be separated:

1. The generic representation must preserve arbitrary JSON array members in order.
2. An optional homogeneous summary can accelerate existing typed operations, but is not an admission rule.
3. Explicit declared field types continue to validate/coerce through the existing validation path.

An acceptable design keeps the homogeneous constructor for callers requesting a typed array and adds an unrestricted constructor for parsed JSON. A mixed summary must remain mixed when subsequent elements are observed. Do not use `Unknown` as an accumulator that first records a conflict and then incorrectly becomes homogeneous again.

Account for empty arrays, all-null arrays, heterogeneous nested arrays, object members, and arrays of objects with different field shapes. Nullability is independent of the non-null kind summary.

Audit all consumers that currently assume `element_type` proves every element's type: mapping operations, bindings, selectors, sort reducers, inference, postings, relation extraction, serialization, and query parameter conversion. A mechanical constructor change is insufficient.

For empty schemas, remove object-root validation. For collections with explicit field declarations, preserve their existing object-record and declared-type contract unless an explicitly supported root declaration exists. This assignment does not require a new schema language for root types. Explicit declared failures must still become diagnosed raw fallback records, with logical fields unavailable.

### 5.3 Numbers outside the existing native domain

General admission must not turn an entire otherwise valid document into raw fallback because a numeric token exceeds `i128` or finite `f64`, or because a nonzero number underflows to zero. The existing JSON dependency already enables arbitrary precision at parsing, but the core conversion currently narrows it.

Add a retained exact JSON-number representation for values the existing number representation cannot faithfully admit under its documented numeric contract. Keep the existing integer/float fast paths for currently supported values. The fallback representation is still a number, not a string, null, or invalid record.

Requirements:

- Preserve the numeric token needed for text extraction and output.
- Support scalar equality and ordering with ordinary numeric values and numeric query literals, including large integer and exponent forms.
- Preserve a consistent total comparison and equal-value hashing across representations. Test boundaries against existing integer/float behavior.
- Extend numeric literal and parameter parsing consistently; a stored large number must not become unqueryable merely because the query literal parser is narrower.
- Do not allocate an expanded decimal string proportional to an exponent such as `1e1000000`. Compare compact significand/exponent representations under configured resource limits.
- Do not add general decimal arithmetic, change temporal units, or reinterpret numeric-looking strings.
- If the accepted inferred-posting codec does not support this number representation, decline that field's typed index and scan it. Never mark partial coverage complete.

The purpose is to preserve valid data and make predicates sound, not to replace all existing floating-point semantics. Document retained ordinary numeric behavior. Exceptional-number handling must not add a heap allocation to every existing integer.

### 5.4 Duplicate names and resource limits

Accept duplicate object names using a documented last-occurrence-wins rule for structured field access, matching the existing map-based decoding model. Whole-document FTS visits every source scalar occurrence, including values in earlier duplicate members, because its corpus is the file's scalar values. Field-specific evaluation uses the resolved logical value. The shared bigram index may therefore contain additional candidates; residual verification decides the requested semantics.

Do not label syntactically valid but deeply nested or exceptionally large JSON as malformed merely because it exceeds a configured execution limit. Return a distinct resource-limit diagnostic/error. Arbitrary JSON shapes do not imply unlimited depth, token size, file size, or process memory. Reuse and document existing limits; avoid uncontrolled recursion or stack overflow.

Malformed JSON remains the existing raw fallback case. Its invalid bytes do not become a fake logical object. The CLI must expose fallback status and diagnostics rather than hiding the file or presenting unavailable logical fields as missing.

## 6. Root paths and heterogeneous selection

### 6.1 Root selector

Add `@root` as an explicit data-root selector in the library query path grammar:

| Expression | Meaning |
| --- | --- |
| `@root = "text"` | Compare a scalar root string. |
| `@root = 123` | Compare a scalar root number. |
| `@root :null` | Apply the existing null test to a present null root. |
| `@root[0] = "first"` | Compare the first member of a root array. |
| `@root[*].title ~ "A"` | Select titles from root-array members under existing array semantics. |
| `@root ?= "text"` | Apply ANY comparison to a root array. |
| `@root.title` | Alias for an ordinary object-root `title` path. |
| `@root["a.b"]` | Address a literal property name containing a dot. |
| `@root[""]` | Address an empty property name. |

`@root` is a selector, not a metadata field or a synthetic property. Preserve the existing `$name` parameter syntax and metadata/time syntax. A real object property named `@root` remains accessible through a quoted field step.

Internally, root access must be represented as a real data root with zero or more path steps. Remove assumptions that a data path always contains at least one field name. Normalize `title` and `@root.title` to the same semantic key where appropriate, while keeping diagnostic display text. Include root/position/quantifier distinctions in prepared, verified, sort, and posting keys.

The lexer, parser, binder, selectors, FILTER evaluator, field-scoped FTS paths, sort bindings, reducers, explain output, and parameterized field operands must agree. A CLI string rewrite from `@root` to an invented field is prohibited.

### 6.2 Selection rules

Preserve the accepted reference evaluator's scalar, ANY, universal, positional, missing, null, and empty-selection semantics. The reference evaluator remains the oracle; extend it before optimizing new shapes.

Mixed arrays must not be filtered down to the requested type before evaluation. For `["text",123]`, ANY equality with `"text"` can succeed; a universal equality must not become true by discarding `123`. Selecting a child field from a scalar is an absent/inapplicable occurrence under the established selector contract, not an empty array. A genuinely present empty array retains the existing vacuous-selection behavior.

Test mixtures containing objects without the selected field, nulls, empty arrays, nonempty nested arrays, and incompatible scalar members. Do not conflate these cases. Preserve observable evaluation errors where the existing language defines them, and do not hide them through candidate pruning.

For arbitrary root containers, return the container payload unchanged. Sorting an unreduced array/object is still governed by the prerequisite's explicit scalar/reducer rules; admitting containers does not imply inventing a lexicographic object comparator or silently stringifying objects.

## 7. Decoded scalar text and shared bigrams

### 7.1 Text policy

Add a documented JSON text policy, conceptually `JsonTextMode::DecodedScalarValues`, and have `obj` select it. Keep the existing logical-string policy available for callers that depend on it. Both policies use the built-in JSON adapter and existing bigram implementation; this is not a separate search engine.

For `DecodedScalarValues`:

| JSON occurrence | Indexed and verified text |
| --- | --- |
| String value | Decoded Unicode string, with JSON escapes interpreted once. |
| Number | Original valid numeric token spelling, excluding surrounding whitespace. |
| Boolean | `true` or `false`. |
| Null | `null`. |
| Property name | Nothing. |
| Object or array container | Nothing by itself; visit its value members recursively. |
| Empty container | No scalar text. |

Numeric text matching is lexical: `1`, `1.0`, and `1e0` are different FTS texts even when a numeric predicate treats them as equal. Preserve exponent spelling, negative zero, and large-number spelling. This avoids lossy conversion and makes whole-document FTS explainable from the source file.

Strings such as `"\\n"` and `"\n"` are distinct after one decoding step. Cover escaped quotes, backslashes, surrogate pairs, control escapes, extended Latin, non-Latin scripts, and an empty string.

Each scalar is a separate text segment. Do not concatenate adjacent values or field names. A phrase spanning two values must not match merely because both contribute grams. Record-level grams remain a conservative union; exact verification checks the relevant segments.

### 7.2 Shared extraction and verification

Expose one consistent adapter traversal/visitor used by construction and exact FTS verification. It must provide decoded scalar text and sufficient path/occurrence information without retaining a second owned copy of every string.

Build and verification must use the same text policy, normalization, case handling, alphabet restrictions, and segment boundaries. An ASCII index must retain the existing conservative fallback behavior for queries whose characters cannot safely restrict candidates. Unicode mode must preserve exact verification semantics.

Do not implement this by serializing the whole decoded object into JSON: that reintroduces keys and syntax, escapes strings again, and loses source number spelling. Do not simply add number grams while leaving verification string-only.

Use the parser's retained token information or a cohesive event-based decode path to make source scalar spans available. Feed grams while values/tokens are available. Avoid a second complete JSON parse for index construction, document-sized clones, or a persistent `Vec<String>` of every scalar. Query payload bytes and decoded projections should reuse the accepted source-access mechanism.

If the current optimized top-level projection assumes an object root, add root-shape-aware parsing and conservative fallbacks. Do not make every ordinary object query use a full projection merely to support scalar roots. Benchmark both cases.

The text-policy identity must participate in source configuration, snapshot compatibility, prepared-query compatibility where relevant, and verified-result identity. A snapshot built with string-only text cannot be reused for scalar-inclusive FTS.

### 7.3 Fallback records and typed predicates

Malformed JSON uses the existing raw-byte text fallback. Do not claim decoded-value matching for malformed text. A declared-type violation also retains the established fallback contract, even if JSON tokenization itself succeeded. Do not leak a rejected logical record into the scalar-text logical population.

Additional scalar grams do not change predicate typing. A numeric `123` can satisfy unscoped FTS `"123"`; it must not satisfy a structured string equality against `"123"` through implicit conversion. A string containing `"true"` and a boolean `true` are distinct logical values.

## 8. Compatibility with the prerequisite optimizations

The prerequisite's proofs were written for homogeneous arrays and object roots. Extend their acceptance coverage before reusing the same access paths for newly admitted shapes.

Required checks:

1. Candidate restrictions remain supersets of reference matches for mixed arrays, scalar roots, explicit root-array positions, and root wildcards.
2. The existing empty-array structural information includes empty root arrays and empty descendants of mixed arrays. Scalar/inapplicable selections are not mislabeled as empty-array witnesses.
3. Whole-document scalar text coverage and logical-string field coverage are separate capabilities. Additional text in the shared index can add false positives but cannot justify a stronger field proof.
4. Inference remains disabled in `obj`. When another caller opts in, mixed observations are reported accurately and incompatible typed index admission is declined. Valid strings, numbers, and booleans must not disappear because an unrelated array member has another type.
5. Presence, explicit null, absent field, inapplicable occurrence, empty selection, and unavailable logical record retain their distinct meanings.
6. MIN/MAX/FIRST/LAST and ordinary dynamic sorting use the accepted comparison/null policy on compatible scalar selections. They do not drop incompatible witnesses to manufacture a sort key. Objects and unreduced nested containers follow the existing documented error policy.
7. Bounded top-k and complete ordered-result caches distinguish a finite page from all-results execution. Cancellation/error still prevents publication of partial complete-result entries.
8. Snapshot/model revisions and source fingerprints reflect representation and text changes. Old data must either migrate through a tested explicit path or be rejected as incompatible.

Do not rebuild the inference or sorting systems. Add only the new representation support, conservative capability decisions, and necessary regression tests.

## 9. All-results execution and memory

### 9.1 Explicit execution scope

Extend the shared core request model with an explicit distinction equivalent to:

```text
Page { offset, limit }
All { offset }
```

Concrete type names should follow the merged API. This is a required semantic distinction. Do not represent all results by setting `limit = usize::MAX`: top-k allocation, offset arithmetic, and cache logic must not rely on a sentinel.

Without `--limit`, use `All`. With it, use `Page`. The streaming scope belongs in the common core execution path, not a second CLI executor.

### 9.2 One execution per query

All-results delivery uses one frozen query, one plan, and one logical scan/candidate traversal. Do not call the page API repeatedly with increasing offsets. That can repeat parsing and sorting, especially with verified caching disabled, and makes query timing misleading.

For default identity order and supported stored ordering, stream verified rows in final order through the accepted bounded worker/reorder mechanism. Skip the offset in that final order. Do not accumulate every match payload or a complete ordered vector merely to iterate it later when a suitable traversal already exists.

For unindexed computed ordering, inspect all required keys before claiming final order. Retain compact identity/key state under the existing byte budget, then materialize bodies in bounded batches. Explain that ordering blocks first-row delivery. If the all-results keys exceed the budget, return a clear resource error recommending an explicit finite limit or suitable configured sort index; do not truncate, spill accidentally to an arbitrary directory, or silently remove sorting.

A finite `--limit` must keep the prerequisite's bounded top-k fast path. No new full-sort path may replace it.

### 9.3 Counts, cancellation, and output pressure

- Successful all-results execution knows exact total matches, including rows skipped by offset.
- A finite page without `--count` may report unknown total; emitted rows are never mislabeled as the total.
- `--limit 0 --count` computes the count without materializing bodies. `--limit 0` without count may return immediately under the accepted API.
- Use ordered rows, not provisional page replacement, for ordinary CLI output. Once a hit is printed, do not later retract it.
- A slow stdout writer applies backpressure. Keep bounded worker batches and payload bytes. Reuse the accepted oversized-record policy and describe its separate limits.
- Ctrl-C requests cancellation, stops scheduling further work, joins/drains safely, and exits 130. Emit a cancelled terminal event only if stdout is still usable.
- Treat a downstream broken pipe as intentional consumer termination: cancel work quietly, emit no fake completion, and exit successfully for that pipeline. Other output failures are nonzero errors.
- No successful terminal outcome after a query execution error or cancellation. No partial membership/order cache publication. Existing valid caches may still be read.

An all-results stream is not a constant-total-memory guarantee for the repository index, a computed sort, or one large record. State the actual bounded working sets and measure them.

## 10. Output protocol and measurements

### 10.1 NDJSON transport

Use one UTF-8 JSON object per physical stdout line. Every event has `protocol_version: 1` and an event `type`. Diagnostics and human-readable progress use stderr. No ANSI formatting or unstructured log lines appear on stdout.

Required event sequence:

```text
run_start
query_start
hit ...
query_complete | query_error | query_cancelled
... next query ...
run_complete | run_error | run_cancelled
```

An index-only operation has `run_start` and a terminal run event. A query-file parse error can emit `query_start` followed by `query_error` with its line number, without entering the engine. Every started query gets exactly one terminal event if the output sink remains usable.

Use monotonically increasing event sequence numbers and monotonically increasing query IDs within the invocation. Sequence numbers express transport order, not rank across different queries. Include the zero-based result position within each query's emitted sequence and the requested offset.

### 10.2 Event contents

`run_start` identifies the executable/core versions, selected root, collection, discovery policy, alphabet, text policy, input mode, and resolved index operation. Do not include absolute paths or secrets unrelated to the selected operation.

`query_start` includes query ID, original input, kind (`structured` or `fts`), source line when applicable, requested result scope, and count policy. FTS text may additionally be included decoded; do not replace the original input needed to diagnose escaping.

Each `hit` includes:

- Query ID and output position.
- Collection, stable record ID, generation, file ID, and relative file path.
- Size, available creation/modification metadata, and record byte range.
- Record class, file status, and diagnostic reference/details when present.
- Available matched-field evidence. Whole-file unscoped FTS must not invent an exact field list if the core does not produce one.
- `data`, containing the entire original JSON document as a JSON value, of any root type.
- `data_available`, distinguishing a real JSON null from an unavailable JSON payload.

Example hit shape, with abbreviated metadata only for illustration:

```json
{"protocol_version":1,"type":"hit","sequence":3,"query_id":1,"position":0,"file":{"path":"nested/example.json","record_id":"nested/example.json","size_bytes":19},"record":{"class":"logical","status":"valid"},"data_available":true,"data":["text",123,true]}
```

Publish the exact final event schema in the CLI documentation and contract tests. Use the actual core status vocabulary or an explicitly versioned mapping, not invented numeric meanings.

### 10.3 Complete source payloads without corrupting NDJSON

Use original source bytes rather than serializing the core's typed enum representation. The returned JSON must retain arrays, scalar roots, undeclared members, number spellings, and duplicate object members. Preserve string value semantics and member order; byte-for-byte outer whitespace is not required.

Because original JSON can span lines, do not insert verbatim multiline bytes into an NDJSON event. Use a validated, streaming JSON compaction writer that removes insignificant whitespace outside string tokens while preserving the document's tokens. Reuse an existing suitable implementation if present. It must not allocate a second document-sized string or rewrite large numbers through `f64`.

The bytes being emitted must be the same revision-qualified payload admitted by the source-access contract. Reuse validation/proof from that read; do not blindly mark arbitrary bytes as valid JSON merely because an older index version parsed successfully. Do not add extra metadata checks per output row beyond the accepted source-access contract.

If a malformed raw fallback record matches, emit `data_available: false`, `data: null`, its diagnostic, and escaped `raw_text` for valid UTF-8 bytes or `raw_base64` otherwise, with an explicit encoding field. If a declared-validation fallback has syntactically valid JSON, its JSON payload can still be emitted with `data_available: true`, while record class and logical availability remain fallback/unavailable. Output payload availability is not logical-query availability.

### 10.4 Terminal measurements

Query terminal events contain:

- Completion state and whether the requested result is complete.
- Emitted row count, observed match count, and exact total or null plus an explicit exactness flag.
- Core work counters: candidates, scanned records, parses, bytes read, and cache usage where the API exposes them.
- Query preparation time, search execution time, sink/output time, total query wall time, and time to first emitted hit when one exists.
- A structured error/diagnostic on failure; partial counts must not be presented as final totals.

Use a monotonic clock and integer nanoseconds in the protocol. Human-readable stderr can show milliseconds. Define boundaries in code and documentation:

| Measurement | Boundary |
| --- | --- |
| Startup | Root opening/discovery plus build or decode/reconciliation, with components reported separately. |
| Snapshot write | Persistence work, separate from query execution. |
| Preparation | Parsing/binding/planning outside the measured execution API, if performed there; no double counting. |
| Search | Core execution elapsed outside synchronous sink callbacks, including required reads, verification, ordering, and payload materialization. |
| Sink | Time inside output callbacks, including event encoding, writes, and backpressure. |
| Query wall | Elapsed from preparation start through query execution and required output flush. |
| First hit | Elapsed from query start until the first hit callback; null if no hit. |

Worker computation can overlap sink time. These are wall-time intervals at the coordinator boundary, not independent CPU costs. Do not label wall-minus-sink as exclusive worker CPU time. If the core does not expose the required execution boundary, add narrow instrumentation rather than fabricating a search duration.

Flush ordinary event output at bounded batch boundaries and before query terminal delivery. Do not flush once per byte or force one operating-system write per field. Terminal metrics need not include the time spent writing their own final line; state that boundary precisely.

Run terminal events contain startup/build/restore/write durations, files/records/fallback counts, query success/error counts, aggregate wall time, and the actual snapshot action (`built`, `restored`, `rebuilt`, `written`, or combinations). A restored index must not be reported as a fresh build.

## 11. Snapshot lifecycle

### 11.1 Required state machine

| Flags | Valid compatible current snapshot | Missing, incompatible, or stale snapshot |
| --- | --- | --- |
| Neither | Ignore it; build in memory. | Build in memory. |
| Write only | Ignore it; build fresh and atomically replace. | Build fresh and atomically save. |
| Load only | Restore, reconcile, execute; no writes. | Fail explicitly; no silent rebuild. |
| Load and write | Restore and execute; no rewrite needed. | Build fresh, atomically save, then execute. |

Distinguish absence/incompatibility/corruption/staleness from access failures. Permission errors, unreadable source inventories, and failed required writes are errors, not reasons to quietly search a different collection or claim a successful restore. When rebuilding after a detectable incompatible/corrupt snapshot with both flags, report the reason and replace it only after a successful build.

`--write-index` is authorization to create the directory and replace this command's snapshot file. It is not authorization to delete arbitrary files in that directory or clear another application's cache.

### 11.2 Paths and compatibility

Treat the optional path as a directory. Preserve the core's alphabet-specific snapshot basename where compatible. Resolve the destination before traversal and exclude it from discovery if it lies inside the selected collection.

The library currently provides explicit save/load primitives, while high-level open defaults to automatic read/write. Add or use a cohesive open policy supporting independent load/write behavior and explicit snapshot location. The CLI should not copy the repository constructor or duplicate reconciliation logic. With neither flag, explicitly disable automatic persistence.

Compatibility must include collection/discovery configuration, reader identity/version, text policy, alphabet, logical model, identity policy, schema/inference/sort configuration, and any new root/number/array representation. Use the established version/hash mechanism; do not create a second ad hoc manifest that can disagree with the snapshot.

Keep startup reconciliation of the complete selected inventory: additions, deletions, and metadata changes must invalidate a stale snapshot. It normally avoids source parsing but still walks/stat-checks the inventory; report that time. Do not add source-content hashing or repeated per-query inventory checks to this assignment. Files changed concurrently with a fresh build remain outside the user's required consistency guarantee.

Use the existing atomic temporary-file-and-rename pattern. Snapshot write failure leaves the previous valid snapshot intact. Temporary files are created in the destination filesystem, cleaned on failure when possible, and never admitted as source documents.

If serialization changes, bump the relevant format/model revision and test old-version rejection. A load-only invocation must explain the incompatibility and the rebuild command. A load-and-write invocation rebuilds according to the table. Do not automatically migrate by trusting unchecked partial structures.

## 12. Performance requirements

The primary costs remain source reads/parsing during construction, candidate verification, sorting, and payload output. Evaluate new work at those boundaries rather than hiding it inside one elapsed total.

Required implementation properties:

- One file read and one cohesive decode/traversal per fresh-build record under the normal reader path; no separate pass just to collect scalar text or infer shapes.
- No field-name inventory or inferred posting construction when inference is disabled.
- No persistent duplicate decoded corpus solely to support FTS verification later.
- Common homogeneous object documents keep fast integer/string/array paths. New generality must not force heap-allocated tagged wrappers around every scalar unnecessarily.
- No concatenation of a file's scalar text into a new document-sized buffer.
- No repeated `search` calls to export all matches.
- No all-hit payload vector before first output. Bound result batches by bytes as well as rows.
- Reading the complete payload is required for output even when predicate projection was small. Reuse that read where possible and report additional payload I/O honestly; do not claim locator-only benchmark costs for full-document output.
- No file I/O in sort comparators. Keep finite-page top-k behavior.
- No per-candidate filesystem inventory reconciliation or speculative live-update machinery.
- No snapshot save when only loading, and no snapshot reads when neither flag requests them.
- No parallel benchmark runs competing for disk bandwidth or page cache.

Record retained index bytes, peak query/output working memory, build allocations where instrumentation exists, and work counters. Additional scalar grams can legitimately increase index size; distinguish that intended corpus change from duplicated buffers or a slower parser.

A repeatable regression above roughly 10% on unchanged ordinary-object workloads requires a written explanation and supervisor review before acceptance. This is an investigation threshold, not permission to ignore smaller clear regressions. Do not claim a target number of milliseconds without measurements.

## 13. Implementation areas

The paths below are based on the inspected architecture. Reconcile them with the merged prerequisite before assigning ownership.

| Responsibility | Location / expected change |
| --- | --- |
| General arrays and exceptional numbers | `crates/core/src/model.rs` and focused value/number helpers. |
| Root paths and parsing | `query_path.rs`, `query.rs`, field-path parsing, normalized request keys. |
| Dynamic truth and declared validation | `reference.rs` and existing selector/coercion modules. |
| JSON text policy and decoding | `source.rs` plus a cohesive JSON reader/text visitor module if warranted. |
| Build-time scalar contribution | `build.rs`, record preparation, source text-coverage interface. |
| Candidate safety | `executor.rs` and capability checks, with existing optimization algorithms retained. |
| Inference and dynamic sorting compatibility | Modules delivered by the prerequisite. |
| All-results execution | Shared request, run/streaming/ranking modules; existing collected API remains compatible. |
| Snapshot policy and revisions | `repository.rs`, `storage.rs`, source configuration fingerprint. |
| CLI application | New `crates/cli` package and workspace member. |
| Documentation | `docs/cli/obj.md`, implementation/acceptance report, root README entry. |

Suggested CLI modules are `arguments`, `query_input`, `execution`, `output`, and `error`. Keep `main.rs` small. Do not put benchmark-specific `WorkloadCase` construction at the center of the public CLI if a normal query request builder exists after the prerequisite. Add a small production request constructor if needed; benchmark fixtures must not define CLI semantics.

All proposed public type names in this document are conceptual unless they already exist. The supervisor freezes actual interfaces before parallel implementation.

## 14. Supervisor, agents, and commit sequence

### 14.1 Workspace and coordination rules

- Read repository and nested `AGENTS.md` instructions and current code guidelines first.
- The supervisor records the prerequisite completion commit and creates an integration branch from that fixed commit.
- Use separate Git worktrees directly under `/home/simon/source`; never use `/tmp` for worktrees.
- Suggested integration branch: `integration/obj-cli`. Suggested worktrees: `/home/simon/source/objekte-obj-json`, `/home/simon/source/objekte-obj-execution`, and `/home/simon/source/objekte-obj-cli`.
- Do not reuse, reset, rebase, or clean another active agent's worktree. Preserve uncommitted user work.
- At most three implementation writers run concurrently. Review/test subagents may inspect or work in their own isolated test branches; they do not edit another writer's branch.
- The supervisor owns shared interface changes, workspace manifests, final snapshot version allocation, conflict resolution, and final acceptance.
- Each implementation agent can use bounded research, test, or review subagents. Require explicit scope and evidence; do not delegate an undefined whole subsystem.

### 14.2 Task and ownership table

| Task | Owner | Dependencies | Required deliverable |
| --- | --- | --- | --- |
| OBJ-00 Baseline and contracts | Supervisor | Completed prerequisite integration | Fixed SHA, interface decisions, ownership map, baseline tests. |
| OBJ-01 General JSON values | JSON agent | OBJ-00 | Arbitrary roots/mixed arrays, strict declared validation preserved, numeric admission support. |
| OBJ-02 Root queries and semantics | JSON agent | OBJ-01 | `@root`, selectors, parameter/literal support, reference tests. |
| OBJ-03 Decoded scalar text | JSON agent with focused review subagent | OBJ-01; final validation after OBJ-02 | One text policy for build and verification, number tokens, string decoding, duplicate-name behavior. |
| OBJ-04 All-results execution | Execution agent | OBJ-00; representation review after OBJ-02 | Explicit All/Page scope, one-pass ordered delivery, bounded sorting/payloads, cancellation. |
| OBJ-05 Snapshot opening policy | Execution agent | OBJ-00; source fingerprint after OBJ-03 | Independent load/write policy, custom directory, versions and atomicity tests. |
| OBJ-06 CLI arguments and input | CLI agent | OBJ-00 | `obj` binary, root configuration, argument grammar and batch reader. |
| OBJ-07 Output and measurements | CLI agent | OBJ-04/05 interfaces; final integration after OBJ-03 | NDJSON, original JSON compaction, metadata, timing, error handling. |
| OBJ-08 Cross-feature verification | Test/review agent, supervisor accountable | OBJ-01 through OBJ-07 | Independent differential and process-level tests; no unreviewed fixes hidden in test branch. |
| OBJ-09 Performance and documentation | Supervisor plus bounded measurement/doc subagents | Integrated code | Sequential measurements, user docs, limitations and exact command examples. |
| OBJ-10 Final merge and promotion | Supervisor | All preceding gates | Reviewed branches integrated, conflicts resolved, tests on final commit, development updated. |

### 14.3 Shared interface checkpoint

Before parallel writers depend on new signatures, freeze:

1. Unrestricted array representation and homogeneous/mixed summary.
2. Exceptional-number admission and comparison contract.
3. Root-path normalization and no-field-step representation.
4. JSON text policy, scalar/token visitor, and coverage declarations.
5. Explicit All/Page result selection and terminal count semantics.
6. Snapshot open policy with resolved path and startup outcome.
7. Output protocol fields and measurement boundaries.

A short committed design note is sufficient. Do not let different agents independently invent conflicting public enums. The JSON agent owns `source.rs` decoding/text changes; the execution agent requests source-fingerprint additions through that owner. Shared `repository.rs`/`storage.rs` integration belongs to the execution agent under supervisor review.

### 14.4 Commit-sized steps

Recommended commits, adjusted only to match current repository conventions:

1. `docs: define obj command and general JSON contracts`
2. `feat(core): admit arbitrary JSON roots and mixed arrays`
3. `feat(core): preserve out-of-range JSON numbers`
4. `feat(query): support explicit JSON root selectors`
5. `feat(json): index decoded scalar values consistently`
6. `fix(core): preserve planner and inference safety for mixed JSON`
7. `feat(core): stream all matching records in one execution`
8. `feat(storage): support explicit snapshot load and write policies`
9. `feat(cli): add obj command and line-based query input`
10. `feat(cli): emit complete JSON results and query measurements`
11. `test(cli): verify general JSON and snapshot workflows`
12. `docs(cli): document obj usage and measured performance`

Not every commit must be released independently, but each must be understandable and testable. Avoid one combined commit containing representation, parsing, persistence, and terminal output changes.

### 14.5 Merge procedure

The supervisor must complete the integration, not stop after agents report branches ready.

1. Review each worker's change set, tests, limitations, and recorded base SHA.
2. Merge general JSON/model/query/text work into the integration branch.
3. Merge all-results execution and snapshot policy, resolving model/source-interface changes explicitly.
4. Merge CLI input/output and process tests.
5. Integrate test/review fixes through the responsible owner or supervisor; rerun affected checks after resolutions.
6. Allocate the final snapshot/model versions once and verify all readers/writers agree.
7. Run final cross-feature validation and sequential benchmarks on the resulting integration commit.
8. If development advanced, incorporate those changes normally, inspect conflicts, and repeat affected validation. Do not discard completed prerequisite fixes.
9. Promote the validated integration result into `development/local-file-search-core` using the repository's normal merge/PR workflow. Do not target `master`, force-push, or rewrite shared history.
10. Record the final commit, merge result, check results, performance report, and remaining explicit limits. Report a real protection/permission blocker if one prevents promotion; do not claim an unmerged branch is the finished deliverable.

When this document is executed, routine integration and the final development merge are part of the assignment. Do not add another discretionary approval pause after the review and checks are complete. Respect any actual repository protection requirements.

## 15. Acceptance tests

### 15.1 Logical values and query behavior

| Case | Required proof |
| --- | --- |
| Object, array, string, number, boolean, null roots | Each is one logical record with unchanged payload. |
| Empty array/object roots | Logical admission succeeds; selection and empty semantics are correct. |
| Mixed array with every JSON kind | No fallback solely because members differ. |
| Nested mixtures and arrays of unlike shapes | Traversal preserves order, positions, and absent/inapplicable distinctions. |
| Unrelated mixed field | Does not make a normal sibling field unavailable. |
| Explicit declared homogeneous field violated | Existing diagnosed raw fallback behavior remains. |
| Root scalar predicates and root positions | Correct matching with `@root`; missing index is not null. |
| Empty and punctuation-bearing object keys | Quoted paths work without normalization collisions. |
| Field named `@root` | Accessible distinctly from the selector. |
| Large integers and exponents | Logical admission, exact exceptional-value comparisons, no overflow/underflow corruption. |
| Resource limits | Classified explicitly; no crash or false malformed classification. |
| Duplicate names | Structured last-wins, file-level scalar FTS all occurrences, payload preserves source members. |
| ANY/universal mixed comparisons | Incompatible members are not silently discarded. |

Compare optimized and reference results for every newly admitted shape, including AND, OR, NOT, negated comparisons, empty selections, explicit null tests, and field operands. Include generated/property-based cases bounded by depth and size. Membership equality alone is insufficient when an evaluation error is observable; compare outcomes as well.

### 15.2 Scalar text and index tests

- JSON escapes, Unicode, backslashes, surrogate pairs, and decoded control characters.
- Property-name-only occurrence does not satisfy decoded scalar FTS.
- Numeric, boolean, and null values participate; objects/arrays do not contribute punctuation.
- Numeric spellings such as `1`, `1.0`, `1e0`, and `-0` follow the lexical text contract.
- Matches do not cross scalar boundaries, even when grams collectively appear in different values.
- Scalar FTS does not introduce implicit conversion in typed FILTER predicates.
- Shared-index false positives are removed by exact verification.
- Both alphabets preserve membership, including non-ASCII needles and short/empty queries.
- Fresh and restored snapshots produce identical results for the same text policy.
- String-only and scalar-inclusive snapshots cannot be confused.
- Malformed JSON and declared-validation fallback retain their established raw-text semantics.

### 15.3 CLI process tests

- Default cwd, explicit `-p`, root directory selection, recursion, and `--no-recurse`.
- Nested duplicate basenames, hidden JSON, `.git`/snapshot exclusions, skipped symlinks, and empty directories.
- No schema configuration reads and no typed/inferred postings in default mode; prove through instrumentation/config inspection.
- Direct structured query, quoted positional FTS, `--fts`, query file, stdin, CRLF/BOM, blank lines, escaped strings, malformed lines, and continued later queries.
- Option ambiguity: a query following bare `--write-index` is not swallowed as its path.
- No default 50-result limit: a fixture with more than 50 matches returns every record.
- All-results export executes once; with result caches disabled, increasing the number of delivered batches does not multiply source parses.
- `--limit`, `--offset`, `--count`, zero limit, and empty result counts.
- Ordered output under different worker completion schedules and computed sorting.
- Original multiline JSON becomes valid single-line event payload without losing tokens; root null differs from unavailable payload.
- Malformed UTF-8 fallback is encoded safely; terminal/ANSI characters in input remain JSON-escaped.
- Every stdout line parses independently; no diagnostic text enters stdout.
- Error after some hits produces a terminal error, never a successful completion; later batch queries continue when safe.
- Ctrl-C, slow sink, broken pipe, other write errors, and cancellation during a computed-sort barrier.
- Peak output memory stays bounded as total result bytes grow, apart from documented index/sort/individual-record bounds.

### 15.4 Persistence tests

Test all four flag combinations against missing, current, stale, incompatible, corrupt, and inaccessible snapshots. Cover custom directories, explicit relative paths, unwritable destinations, failed atomic replacement, and exclusion of the index directory from discovery.

Add, remove, rename, and modify source files between invocations. Verify load-only rejection and load-plus-write rebuild, without a query silently running against an old inventory. Snapshot restore must not reparse unchanged source JSON merely to reconstruct the index. Do not add stronger metadata/content freshness claims than the existing reconciliation contract supports.

Verify that neither-flag and load-only modes leave snapshot files untouched, and write-only performs a fresh build even when a valid snapshot exists.

## 16. Measurements and delivery gates

Measure the merged prerequisite as the baseline, then the final implementation on the same machine, toolchain, corpus, alphabet, worker counts, cache settings, and output destination.

Use:

1. The existing Musenalm corpus for ordinary object performance and the accepted query suite.
2. A reproducible general-JSON fixture covering mixed/root/escaped/scalar cases.
3. A generated large-match workload to expose output buffering, repeated scans, and unbounded key retention.

Record fresh build, restore decode, inventory reconciliation, snapshot write, selective FTS, selective dynamic text FILTER, numeric scan, finite sorted page, all-results default-order export, and computed sorted export under budget. Include time to first result, total elapsed, counters, peak memory, and snapshot/index size.

Run both alphabets where relevant. Compare compatible old/new text policies separately from scalar-inclusive mode so additional indexed content is visible. Report warm/cold cache conditions accurately; do not call filesystem-warm runs cold. Run performance measurements sequentially and repeat enough to distinguish variation from a meaningful regression.

Run the workspace's prescribed formatter, lint, unit/integration tests, and build checks. Discover the current required commands from repository configuration rather than assuming obsolete flags. The new `obj` package must build in the normal workspace and have a release invocation documented. Run the accepted core/query regression suites on the final integration commit; do not claim prior branch test results cover a later conflict resolution.

Performance acceptance is not just lower wall time. Require evidence that new admission support did not introduce extra parses, that exported batches do not restart queries, and that output does not retain all document bodies.

## 17. Required final deliverables

- Working `obj` executable and workspace package.
- General JSON logical admission, explicit root paths, and decoded scalar text policy in the library.
- Explicit all-results query scope with ordered bounded delivery.
- Explicit snapshot load/write policy and custom index directory support.
- Process-level CLI tests and differential core tests for the new shapes.
- `docs/cli/obj.md`, including examples, query-file grammar, event schema, timing boundaries, snapshot table, fallback behavior, and memory limits.
- A concise implementation/acceptance report with final SHA, prerequisite SHA, checks, benchmark conditions/results, and known limits.
- Reviewed integration and final development-branch merge, or a specific externally imposed blocker with the prepared integration result identified.

Definition of done: a user can enter an arbitrary directory of JSON documents, run `obj --fts TEXT` or a structured query without supplying field names in configuration, receive every matching complete document with metadata and timings, save the index, and restore it later. Arrays and scalar roots are logical data, mixed arrays do not force fallback, explicit declared validation remains strict, and large outputs do not cause hidden repeated scans or retention of all document bodies.

## 18. Supervisor starting instruction

> Execute this plan against the completed optional-indexing/sorting/streaming integration. Pin that commit and preserve its behavior. Create isolated worktrees under `/home/simon/source`, freeze the shared contracts, assign the JSON, execution, and CLI work with bounded review/test subagents, and complete implementation, review, final integration, validation, and promotion to `development/local-file-search-core`. The executable is `obj`. Do not stop at planning, skeleton code, unmerged worker branches, or an example with application-specific fields. Resolve routine implementation choices within the contracts above and report measured performance limitations explicitly.

## Reference material

- [Inspected development tree](https://github.com/Simon-Martens/objekte/tree/f374101b4839f28199a99677118ac96869203bb0).
- [Value representation](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/core/src/model.rs).
- [JSON reader and source contract](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/core/src/source.rs).
- [Query paths](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/core/src/query_path.rs).
- [Repository opening and results](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/core/src/repository.rs).
- [Snapshot storage](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/core/src/storage.rs).
- [Prerequisite implementation specification](https://chatgpt.com/library?file_id=libfile_8f10deace404819196483e37820231c4&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2).
