# Objekte automatic indexing, schema configuration and interactive CLI

Implementation supplement — 7 November 2026

## 1. Assignment and precedence

Extend the optional inference and CLI work with four requirements:

1. In `AutoIndex` mode, automatically create ordered B-tree postings for decoded string values whose UTF-8 representation is **at most 64 bytes (512 bits)**, independently of the low-cardinality string heuristic. This includes UUID strings in their usual 36-character hyphenated form.
2. Add **`obj --auto-index`**, which enables both field inference and automatic indexing for the command's collection during opening/construction.
3. Add **`obj --schema FILE`**, which loads an explicitly selected JSON Schema as the collection's type declarations/hints and manual index configuration.
4. Add **`obj --tui`**, reusing the current TUI's appearance, search inputs, options and applicable keyboard controls, with a restricted result table and no links.

This supplements `objekte_optional_indexing_sorting_streaming_specification.md` and `obj_cli_implementation_plan.md`. Use their completed or coordinated implementation as the base. Do not start a competing inference system, JSON reader, query evaluator, or CLI. This document is a work order for the implementation agents; preparing it does not start implementation.

The earlier 128-bit/16-byte limit is superseded. A UUID contains 128 bits of binary information, but its ordinary hyphenated textual representation needs 36 ASCII/UTF-8 bytes. Use an inclusive **64 decoded UTF-8 byte** threshold, also accommodating the usual braced and `urn:uuid:` forms. Keep UUIDs as ordinary strings unless explicitly declared otherwise; do not require UUID parsing or binary conversion to qualify. This measures value bytes, not the total size of a B-tree key, node, or posting list.

`obj` is also a general integration test and benchmark frontend for the library: the same collection and query corpus must be runnable without hints, with manual schema indexes, with automatic indexes, and with both. The TUI provides interactive inspection of those same capabilities; machine-readable mode provides reproducible measurements.

This supplement changes the following earlier requirements:

| Earlier requirement | Required amendment |
| --- | --- |
| AutoIndex admits Boolean, exact numeric and low-cardinality string postings. | Add short-string B-tree postings regardless of cardinality or repetition. |
| A long string or mixed observation can disqualify the entire provisional string index. | Such observations can disqualify the low-cardinality route or complete-field proof; they must not disqualify otherwise eligible short-string values. Retain explicit coverage and a conservative residual domain. |
| Complete indexes are preferred; partial indexes are optional. | Coverage-aware short-string postings are required when a field also contains long strings or other values. An entire-field exact proof remains optional and requires evidence. |
| CLI inference is disabled. | Disabled remains the default. `--auto-index` explicitly selects `AutoIndex`. |
| No schema file is loaded. | No implicit schema discovery. `--schema FILE` explicitly loads one schema for the CLI collection. |
| The original CLI plan excludes a TUI. | `--tui` is now required and explicitly supersedes that exclusion. |
| CLI output is always NDJSON and queryless interactive use is excluded. | NDJSON remains the noninteractive protocol. `--tui` selects terminal rendering and permits startup without a query. |

All other contracts remain in force: schema-free admission, arbitrary valid JSON support from the CLI plan, decoded scalar FTS, dynamic FILTER semantics, sorting, streaming, explicit snapshot policy, bounded resources and supervisor-owned final integration.

The existing Boolean/numeric admission rules remain unchanged. This supplement does not require partial Boolean/numeric indexing, relation inference, schema-file updates, automatic sort columns, filesystem watching, or a background indexing service.

## 2. Invariants

- With inference disabled, do not allocate inference-owned field catalogues, short-string dictionaries or inferred posting builders.
- `ReportOnly` reports observations and admission advice without constructing/publishing the full short-string postings. A high-cardinality report may use explicitly bounded or incomplete distinct statistics.
- `AutoIndex` adds accelerators; it does not declare field types or create validation constraints.
- With the same explicit schema configuration, sources and query must produce the same membership, counts, ordering, payloads and observable errors in Disabled, ReportOnly and AutoIndex modes. A supplied schema can introduce the existing explicit declaration/validation contracts; compare schema-free and schema-configured execution only where those contracts are equivalent.
- An explicit indexing prohibition remains authoritative. A compatible explicit posting is reused rather than duplicated.
- Values rejected by an optional indexing budget or unsupported representation remain ordinary queryable values. No record or result may be omitted to satisfy an indexing limit.
- Shared bigrams remain enabled under the existing configuration. A B-tree does not replace substring/FTS candidate generation.
- Inference observes logical string values. JSON numbers, booleans and null rendered as text for FTS must not also become inferred string values.

## 3. Short-string admission

### 3.1 Predicate and examples

For an actual logical string `value`:

```text
eligible_short_string(value) := value.as_bytes().len() <= 64
```

Use the existing decoded value; no second JSON parse or allocated UTF-8 conversion is needed when it is already represented as a Rust string.

| Decoded logical value | UTF-8 bytes | Short-string admission |
| --- | ---: | --- |
| Empty string | 0 | Yes |
| `550e8400-e29b-41d4-a716-446655440000` | 36 | Yes, without UUID-specific conversion |
| Braces around that UUID | 38 | Yes |
| `urn:uuid:` followed by that UUID | 45 | Yes |
| 64 repetitions of `a` | 64 | Yes |
| 65 repetitions of `a` | 65 | No through the short-string route |
| 32 repetitions of `ü` | 64 | Yes |
| 33 repetitions of `ü` | 66 | No through this route |
| 16 repetitions of `😀` | 64 | Yes |
| JSON `"\u0041"`, decoded as `A` | 1 | Yes |
| Decoded string `null` | 4 | Yes |
| JSON null | Not a string | Use existing null semantics |

No minimum number of records, minimum repetition, enum ratio, or 64-distinct-value threshold applies to short strings. A field with 100,000 distinct 36-byte UUID strings qualifies when its resource budget permits. A field observed in one record can qualify. Empty and whitespace-only strings qualify.

Test length on the original decoded string before any comparison-specific transformation. Do not trim, lowercase, normalize Unicode, parse a number, or parse a date to decide admission. Comparison-key transformations, if needed by the actual predicate semantics, are a separate operation and their complete memory cost must be counted.

The library policy exposes this threshold in bytes, with a default of 64 and a versioned identity. Reuse the existing policy naming conventions; a suitable conceptual name is `short_string_max_utf8_bytes`. No CLI threshold override is required. If the implementation allows a different limit, persist it and use storage capable of representing it; never silently truncate a value to fit an inline key.

### 3.2 Field and collection scope

Create a per-collection, per-canonical-path ordered value dictionary with record-ID postings. Do not put unrelated fields in one B-tree merely because their values are short.

Cover the inference system's supported canonical logical leaf paths, including nested object/array leaves and the general JSON root path. An eligible string root belongs to `@root`; a root array remains one record. Repeated array occurrences add at most one membership for a given value and record.

Do not enumerate every possible positional selector as a separate index. An index over a canonical array-expanded path can be used for a narrower position only with a conservative proof and residual verification. Preserve the distinction between object keys, array steps, literal dots in keys and numeric-looking property names.

Long values and mixed types are not field-wide vetoes for this route. For example:

```json
{"code": "A17"}
{"code": "This descriptive value is intentionally longer than sixty-four UTF-8 bytes."}
{"code": 17}
{"code": ["B18", "Another descriptive value is intentionally longer than sixty-four UTF-8 bytes.", null]}
```

Retain B-tree memberships for `A17` and `B18` at the appropriate canonical selection. Report the field as mixed/partially covered. Its long, numeric, null and array states remain available to the ordinary evaluator. This example authorizes string-domain acceleration, not a declaration that `code` is a string field.

If a reader cannot expose decoded strings or complete selector-state coverage, report that capability limitation. Do not infer logical strings from raw fallback text or property names.

### 3.3 Interaction with low-cardinality admission

Keep the existing bounded low-cardinality policy and configuration for compatibility. Its minimum observations, repetition ratio, length bound and dictionary budget remain independent. With the earlier default maximum of 64 bytes, its admitted strings now fall inside the short-string domain, so it must not cause duplicate dictionaries or an additional observation pass. Only an explicitly configured larger low-cardinality length limit can admit values beyond the short-string threshold through that route; do not silently raise it.

For an eligible logical string, the combined policy is conceptually:

```text
admit when short_string(value)
       or the existing low_cardinality_policy admits the field/value
```

Use one compatible ordered dictionary per path where possible, with admission reasons and coverage metadata. Do not duplicate the same short-string key and record memberships in separate enum and short-string indexes. If existing storage requires separate sections, provide one planner capability and account for any duplication explicitly.

When the low-cardinality candidate is rejected, keep the qualifying short-string entries. Remove any now-ineligible longer entries safely, or retain them only under a separately valid, documented admission rule. Do not rebuild the same rejected low-cardinality candidate repeatedly as more records arrive.

## 4. Ordered dictionary and comparison semantics

Reuse the existing typed posting and `ScalarKey` infrastructure where it can supply an ordered B-tree. This is an actual ordered index: a hash table, Bloom filter, or 128-bit digest is not a substitute. Retain exact values and record memberships.

The supervisor must verify the existing string equality/range semantics before freezing the key codec. A B-tree is usable only when its key equivalence and ordering agree with the bound operation:

- Raw UTF-8 ordering is suitable only for a predicate whose semantics use that ordering.
- Case-insensitive equality requires an existing compatible equivalence representation, or a conservative lookup plus exact residual verification that cannot omit equivalent values.
- Collated range comparison requires a compatible ordered representation. Binary byte intervals must not be substituted for ICU or another collation.
- Transforms or custom operations without a proven index contract retain their existing fallback.

For the initial implementation, prefer the established exact string key representation over introducing an independent compact-key subsystem. Avoid a new heap allocation for every repeated occurrence; intern each retained distinct value and reuse the existing posting builder.

A fixed-width key optimization is optional. If used, retain the actual byte length, preserve embedded NUL bytes, and compare the logical byte sequence correctly. Zero padding alone cannot distinguish `"a"` from `"a\u0000"`. Native-endian integer order and length-first order do not automatically implement lexicographic string order. Persist a defined codec rather than raw memory layout.

The required query benefit is selective ordinary string equality and compatible ordered predicates on short strings. Add range access where the existing comparison representation supports it; preserve and explain fallback for incompatible operations. Do not claim that the presence of a B-tree accelerates arbitrary contains, regex or locale-sensitive sorting.

This supplement creates no automatic resident sort columns or composite orders. The prior broader-sorting implementation remains responsible for `SORT BY`; consuming an existing compatible dictionary must not bypass its collation, array and tie-breaking contracts.

## 5. Coverage and query planning

### 5.1 Distinguish indexed values from complete predicate coverage

An index containing every eligible short string can still have incomplete coverage of a field predicate. Store enough information to distinguish:

- Indexed short-string memberships.
- Complete versus incomplete observation of the relevant source generation.
- Records whose complete selection is represented by the index and its structural facts.
- Records with unrepresented values or unsupported selection/operation behavior.
- Missing, explicit null, present empty selection, array state and unavailable logical data.

Extend the existing coverage types rather than adding an unrelated second planner. Persist coverage alongside postings. If the implementation cannot establish a complete-selection proof, classify the record conservatively as unresolved.

The `records_with_null_occurrence` statistic is not the exact `:null` predicate bitmap. Keep the earlier distinction, including terminal arrays containing null. Empty ancestors can imply a present empty descendant selection even if that descendant path is discovered in a later record or worker batch; retain the existing structural reconciliation.

### 5.2 Minimum safe execution strategy

Use the following as a conservative baseline:

```text
I = the input record domain at this predicate
F = records in I for which the bound predicate's full selection is covered
R = I minus F
P = conservative predicate candidates from the B-tree and structural facts within F

candidates = P union R
```

`F` is specific to the path, operator, operand type, quantifier, representation and available error proof. It is not simply the set of records with any indexed string. Conservatively leaving a record in `R` is correct. The earlier planner's unavailable/Boolean composition rules continue to apply.

Verify candidates through the ordinary evaluator unless the existing exact-proof framework establishes the complete result. If an error could be suppressed by skipping a record, keep that record in `R` or decline the access path. No whole-collection metadata reconciliation is added per query.

This strategy lets an ordinary all-short-string field accelerate strongly while safely scanning the exceptional records of a mixed field. A field whose every record mixes short and unindexed values may gain little from this initial strategy. Report that limitation and cost the actual remainder; do not pretend those records were eliminated.

More selective use of indexed witnesses inside partially covered records is optional. It requires its own quantifier/error proof and independent tests; it is not required to deliver this supplement.

### 5.3 Required operator safeguards

- **Positive equality:** Use compatible lookup candidates. Unrepresented values are not automatically false, especially under transformed/collated equivalence.
- **Ranges:** A long value can lie between short bounds. For `code > "A"`, the value formed by `A` followed by 64 zero characters (65 bytes) may match; it must survive in the remainder.
- **ANY:** One compatible witness may prove membership only when the evaluator's error and unavailable contracts permit that shortcut. The conservative baseline may still verify it.
- **Default/ALL:** One indexed matching occurrence does not prove that every occurrence matches. Preserve empty-selection truth and unindexed counterexamples.
- **Negation and inequality:** Never complement a partial or approximate posting over the full collection. Require complete compatible coverage or retain the unresolved remainder and residual.
- **Missing and null:** Use exact selector-state information only. Do not derive missingness as “absent from the string B-tree.”
- **Parameter changes:** A prepared query executed with a string, then a number, must bind and select access paths correctly each time.
- **Field-to-field comparisons, relations and transforms:** Retain established planner capability checks; short-string admission does not make these automatically indexable.
- **AND/OR/NOT:** Preserve the existing dynamic and unavailable semantics, including metadata branches that select fallback records.

Where the remainder is broad, scanning may be cheaper. Keep cost-based selection and expose why the B-tree was selected or rejected.

## 6. Resource accounting and construction

“All short strings” removes the low-cardinality gate; it does not remove explicit resource limits or user opt-outs. Within those limits, every eligible value at a supported observed path must be represented. A limit must produce a visible admission decision rather than a silently truncated index.

A value of at most 64 bytes can still have substantial overhead: distinct-key storage, B-tree nodes, posting lists, structural bitmaps and worker buffers. High-cardinality identifiers can therefore materially increase build time, memory and snapshot size. Measure this separately from low-cardinality strings, including the increase from 16-byte keys to 36-byte UUIDs and 64-byte boundary values.

Required implementation properties:

1. Observe values during the existing decode/traversal. Do not reopen or reparse the source corpus for short-string indexing.
2. Reuse canonical paths and decoded values. Do not clone a document or retain the full decoded corpus for inference.
3. Deduplicate repeated value/record memberships using the accepted builder or reusable scratch state.
4. Account for final indexes, distinct text, node overhead, worker-local contributions, coverage facts and merge-time duplication under the existing inference budgets.
5. Use bounded per-worker batches and deterministic merges. Do not serialize every occurrence through a global mutex protecting a B-tree.
6. Preserve the earlier bounds on path count, nesting and per-record contributions. A skipped or incomplete path cannot acquire a complete-coverage label.
7. On budget exhaustion, deterministically reject/drop the affected optional accelerator and release its buffers. Retain ordinary query fallback and report the reason. Do not silently keep only the first N values and call the field complete.
8. Retain the existing deterministic admission policy across worker counts and batch boundaries. Any policy amendment must be versioned and tested.

Existing small-dictionary cardinality limits must not continue to disqualify short strings under a different name. Genuine per-field/total memory limits still apply and must be distinguished from cardinality heuristics in reports.

No new CLI memory/threshold switches are required in this supplement. Reuse the accepted library policy and resource settings; record their concrete defaults in the design note and benchmark report.

## 7. `obj` options, schema configuration and TUI

### 7.1 Argument behavior

Add one Boolean option:

```text
--auto-index
    Infer field observations and build automatic Boolean, numeric,
    low-cardinality string, and short-string B-tree indexes.
    Short strings contain at most 64 decoded UTF-8 bytes.
```

It takes no value and needs no short alias. The mapping is:

| Invocation configuration | Core inference mode |
| --- | --- |
| Option absent | `Disabled` |
| `--auto-index` present | `AutoIndex` |

Map the option into the core's opening/build configuration before construction or restore. Do not run a second CLI-owned inference pass. Do not emulate the option by declaring inferred fields in `Schema`.

The option applies to the single `files` collection and all queries in the invocation. Build/open once. It works with a positional query, `--fts`, `--query-file`, explicit pagination, all-results streaming, recursion options and both snapshot flags.

`--auto-index` alone does not read schema files, enable implicit persistence, change result limits or change JSON admission. `--schema FILE` separately enables explicit schema configuration. The earlier no-query rule remains outside TUI mode: `obj --auto-index` alone is a missing-query error; `obj --auto-index --write-index` is an index-only operation. `obj --auto-index --tui` opens the interactive screen without requiring an initial query.

### 7.2 Examples

```bash
obj --auto-index 'code = "A17"'
obj --auto-index -p ./documents 'year >= 1800 && year < 1900'
obj --auto-index --query-file queries.txt
obj --auto-index --fts Goethe
obj --auto-index --write-index
obj --auto-index --load-index 'code = "A17"'
obj --auto-index --load-index --write-index --query-file queries.txt
```

Compile/test the structured examples against the integrated grammar. The FTS example enables inference for the repository; it does not promise that this particular FTS query uses a B-tree.

### 7.3 Output and measurements

Extend existing startup/terminal NDJSON metadata with the requested/effective inference mode and an aggregate automatic-index summary. Include counts of admitted short-string fields, complete/partial capabilities, and budget/incompatibility rejections. Reuse the established report for detailed per-field explanations; do not print a large inference report as unframed stdout text.

Distinguish construction from restored indexes. Report additional inference/index-construction measurements where available without double-counting subphase time already included in build duration. Query timing boundaries and full source payload output remain unchanged.

Document that AutoIndex trades additional construction memory/time for faster suitable structured queries, and that memory limits can decline optional indexes without changing results.

### 7.4 Explicit JSON Schema: `--schema FILE`

#### Loading and scope

`--schema FILE` takes one required local filename. Relative filenames resolve against the invocation's working directory, independently of `--path`. This is local file loading, not a network upload. Read and compile it once before building/restoring the collection. A missing filename, unreadable file, invalid JSON or unsupported configuration produces a clear startup error with the filename and relevant schema location.

The schema describes the CLI's one `files` collection. Collection root, recursion, JSON reader and relative-path record identity remain controlled by the CLI. Do not require `.objekte/collections`, an `x-objekte` wrapper, or an application-specific schema layout. If an existing schema contains repository metadata, do not use it to change the CLI root, create additional collections or replace its identity policy; report ignored repository-only metadata clearly.

Exclude the explicitly supplied schema file from source discovery if it resides inside the collection. No schema is discovered or loaded when the flag is absent. Other JSON files retain ordinary discovery behavior. Neither the schema nor source files are modified.

Reuse/extract the existing schema translation into a cohesive adapter that accepts a parsed schema plus caller-supplied collection configuration. The inspected adapter is `crates/tui/src/schema.rs`; its current loading of repository collection directories must not be copied into the general CLI. Share schema interpretation between callers instead of creating two inconsistent implementations.

#### Types and manual indexes

Use the established Objekte JSON Schema annotations:

- Standard `properties`, nested objects, arrays/items and supported scalar `type` declarations provide logical field types. Preserve supported required/nullability contracts.
- Property-level **`"index": true`** requests a typed posting explicitly.
- Root-level **`"x-index-paths": ["path", ...]`** requests manual postings using the established path grammar.
- Explicit **`"index": false`** prohibits automatic indexing for that path. Preserve the distinction between absent annotation and explicit false.
- Existing supported **`x-sort-indexes`** are explicit sort configuration. Reuse the accepted sort adapter and reject unsupported entries; do not silently flatten a composite order into unrelated single-column orders.

`index`, `x-index-paths` and `x-sort-indexes` are Objekte extensions, not standard JSON Schema indexing keywords. Document this and provide a runnable fixture:

```json
{
  "type": "object",
  "properties": {
    "id": { "type": "string", "index": true },
    "year": { "type": "integer" },
    "active": { "type": "boolean", "index": true },
    "body": { "type": "string", "index": false }
  },
  "x-index-paths": ["year"]
}
```

This manually indexes `id`, `year` and `active`, and prevents automatic typed indexing of `body`; ordinary shared FTS is unchanged. The source field `id` does not replace the CLI's relative-path record identity.

Manual indexes do not require `--auto-index` and are not limited by the automatic 64-byte string threshold or enum heuristics. They remain subject to the library's supported type/comparator and explicit resource contracts. An unsupported or failed explicitly requested index must produce a diagnostic/error according to that contract; do not silently describe a declined request as built.

Use these precedence rules:

| Configuration | Result |
| --- | --- |
| Neither flag | Empty schema, inference disabled; ordinary bigrams. |
| `--schema FILE` only | Explicit types and manual indexes; no automatic inference/index admission. |
| `--auto-index` only | Existing schema-free AutoIndex behavior. |
| Both | Explicit declarations/manual indexes plus AutoIndex for other eligible paths; explicit prohibitions remain authoritative. |
| Compatible explicit and inferred index | Reuse the explicit index; no duplicate dictionary. |
| The same path has `index: false` and a manual request in `x-index-paths` | Configuration conflict; reject rather than relying on parsing order. |

Supplying types does not automatically request an index on every declared property. An absent index annotation is unspecified and may be considered by AutoIndex; it is not an explicit prohibition. Inferred observations must not overwrite explicit types or convert string UUIDs/numeric-looking strings without an existing explicit conversion contract.

#### Schema semantics and supported subset

The flag supplies the core's existing explicit field contracts and physical index requests. It is not a new promise of complete JSON Schema validation. Preserve and document supported validation behavior, including how a record violating an explicit declaration is diagnosed and, where the core requires it, retained as fallback. Undeclared data remains dynamic under the accepted schema contract. Never treat a hint as proof of complete typed coverage without the library's validation/coverage mechanism.

Keep arbitrary JSON admission with no schema. With a schema, support the root/array/scalar forms the accepted adapter and general logical model can express. Do not silently choose the first non-null branch of an unsupported union, ignore a meaningful array schema, or coerce invalid values to satisfy a type.

Resolve local fragment references such as `#/$defs/identifier` when needed to interpret supported types/index annotations, with bounded resolution and clear handling of cycles. Do not fetch remote schemas or external resources implicitly. Unsupported references or constructs that prevent safe translation must fail with a precise message. Unsupported validation-only keywords must follow an explicit documented policy and must not be advertised as enforced.

Relation annotations retain the core's supported declaration contract, but the CLI still opens one collection. Never open additional repositories because a schema names a relation target. Reject unavailable target requirements explicitly; do not invent links. The TUI's no-link presentation policy applies even if a valid relation is configured.

Examples:

```bash
obj -p ./documents --schema ./document.schema.json 'year >= 1800'
obj -p ./documents --schema ./document.schema.json --auto-index --query-file queries.txt
obj -p ./documents --schema ./document.schema.json --write-index
obj -p ./documents --schema ./document.schema.json --auto-index --tui
```

### 7.5 Interactive mode: `--tui`

#### Existing appearance and controls are the reference

`--tui` opens an interactive terminal interface based on the existing `crates/tui` implementation. The source inspected for this revision is `f374101b4839f28199a99677118ac96869203bb0`. The supervisor must compare any newer UI changes before choosing the implementation baseline.

Reuse the existing theme, spacing, header/footer, selection marker, input editing, overlays, preview/detail presentation and mouse/navigation behavior. Keep the separate **Search** and **Filter** inputs and the `Aa` case-sensitivity and `.*` regex controls. Do not replace the interface with a new TUI framework or redesign its interaction model. Extract reusable components or introduce a small single-collection display configuration rather than copying the whole application.

Preserve these current bindings, with their existing focus-sensitive meaning:

| Keys | Required behavior |
| --- | --- |
| `Tab` / `Shift+Tab` | Cycle Search, Filter and results focus. |
| `Enter` | Execute/confirm in query inputs; open the selected record from results; expand/collapse in record detail. |
| `Esc` | Existing close/back/focus behavior; during loading, cancel/exit as currently supported. |
| Arrow keys, `PageUp`, `PageDown`, `Home`, `End` | Existing input, row and detail navigation. |
| `Alt+C` | Toggle case sensitivity in Search. |
| `Alt+R` | Toggle regex in Search. |
| `Alt+S` | Sort controls. |
| `Alt+L` | Column controls, limited to the query-derived optional columns described below. |
| `Alt+Left` / `Alt+Right` | Scroll optional result columns; navigate adjacent results in detail. |
| `Ctrl+K`, `F1` | Actions and contextual help. |
| `Ctrl+P`, `Alt+T` | Preview toggle and existing light/dark theme toggle. |
| `Ctrl+R`, `Ctrl+B`, `Ctrl+S` | Query history, saved queries and save-query action. |
| `Alt+E`, `Alt+A` | Existing explicit record/all-results export actions. |
| `/`, `n`, `N`, `y`, `Y`, `r` in detail | Find, next/previous find, copy value/record and raw view. |
| `Ctrl+O` | Existing collection action, restricted to the single CLI collection; no repository discovery. |
| `Ctrl+Q` | Quit. |

Keep the existing editor shortcuts and international keyboard handling from `input.rs`, including its AltGr treatment. Preserve contextual dispatch rather than interpreting these shortcuts globally. Relation-specific actions are the explicit exception: disable/remove them from this mode and its help, without reassigning their keys to unrelated actions.

The current Search options apply to FTS and must be wired to the same library regex/case configuration as the original UI. They do not silently rewrite structured FILTER comparison semantics. Invalid regex/filter text appears as an in-screen error; the terminal remains usable. Keep the current debounce behavior unless measurement justifies a documented adjustment.

#### Result columns

The result table has exactly three permanent logical columns:

| Column | Content |
| --- | --- |
| `valid` | Existing colored validity/status dot. No numeric or text status column is added. |
| `id` | The core record ID under the CLI's relative-path identity policy. |
| `match` | Actual matched text/value or verified evidence, with the existing highlighting style. |

There is **no collection column and no default schema, inferred, title, filename, relation, or application-specific column**. Selection/focus markers are row decorations, not additional data columns. Schema fields and inferred fields must not populate the table automatically.

Insert optional field columns between `id` and `match` when the active successfully bound query filters by or sorts on those fields:

1. Derive the column set from the parsed/bound FILTER and sort expressions, never from regex extraction of query text or a scan of the source corpus.
2. Include fields referenced in nested Boolean branches and both operands of field-to-field comparisons. A metadata filter can also contribute its actual metadata field.
3. Add filter fields in first-appearance order, then sort expressions in sort-priority order. Deduplicate identical canonical expressions.
4. Do not confuse the source property `id` with the permanent record identity column. If filtering on the source property, add a clearly labeled field column, for example `@root.id` using the accepted canonical syntax. Only the exact record-identity metadata expression can reuse the permanent `id` column.
5. Use canonical path/transform/reducer labels. A sort such as `MIN(prices[*])` shows the actual reduced key; raw `prices` and `MIN(prices[*])` are distinct expressions when both are used.
6. Render missing, explicit null, unavailable and empty arrays distinctly using the existing display conventions. Preserve actual types and array values; do not invent a scalar for an unreduced multivalued path.
7. Clearing a field from both filter and sort removes its column. Typing an invalid query must not pair old rows with misleading new column labels; keep a consistent previous result/query pair or show an empty/error state.
8. Retain horizontal scrolling and clipping for narrow terminals. The column picker may hide/show current query-derived fields, but cannot add unrelated schema fields; newly required fields are visible by default. Permanent columns cannot be removed. Do not let old saved column preferences add arbitrary columns in this mode.

Examples of the logical column order:

| Active query | Columns |
| --- | --- |
| Search for `Goethe`, no filter/sort | `valid`, `id`, `match` |
| Filter `year >= 1800` | `valid`, `id`, `year`, `match` |
| Filter `year >= 1800 SORT BY title` | `valid`, `id`, `year`, `title`, `match` |
| Filter `year >= 1800 SORT BY year DESC` | `valid`, `id`, `year`, `match` |

Sorting must support undeclared fields through the prior broader-sorting work. Keep `Alt+S` and the existing overlay style, but allow entering/selecting a field or supported sort expression without requiring a schema or registered sort column. The Filter input also accepts the integrated inline `SORT BY` grammar. Honor the library's existing conflict rule if both inline and separate sorting are provided; do not silently apply contradictory orders.

#### Validity and match presentation

Reuse the existing green valid/red broken-or-unreadable dot and diagnostic access, extended only as required by the accepted schema-status model. Explain what “valid” means under the active configuration: successful logical decoding and configured checks, not an assertion that every JSON Schema keyword was evaluated. Source/schema diagnostics remain inspectable in detail.

`match` must show what actually matched, not the old heuristic fallback to `preferred_title`, `title`, `name`, or the first string property. Use query evidence and compatible decoded field projections. For text/regex matches show a bounded snippet around an actual match; for a structured predicate show a relevant verified value where available. Pure sort/all-records requests, negative or absence predicates may have no positive value witness: render an empty/neutral marker rather than fabricate one. Evidence absence is not a new query failure.

Use the library's path selection for nested arrays, escaped keys and root values; the inspected renderer's simple `split('.')` lookup is insufficient. Highlight using offsets in the displayed decoded representation, not raw JSON byte offsets. Preserve Unicode/case-fold/regex correctness and distinguish raw fallback snippets when applicable.

#### No links

Disable relation/link presentation and navigation in this TUI mode: no outgoing/backlink sections, target activation, link-following history, implicit relation fetching, URL opening or terminal hyperlink emission. IDs and URL-shaped source strings are plain text. Detail/preview still show the record's original fields, including relation-valued strings as ordinary data; opening a result and moving to an adjacent result are retained record navigation.

Do not call `refresh_relations` or equivalent background expansion just to populate a hidden panel. Remove relation-only help/actions and section shortcuts from this mode. Preserve the existing full TUI's link behavior for its other callers.

Render untrusted source strings as safe terminal text. Do not allow embedded control/escape sequences to create terminal actions or hyperlinks. Retain explicit user-triggered copy behavior through the existing clipboard mechanism.

#### Startup, execution and terminal lifecycle

- `obj --tui` is valid without a query and opens with the existing empty-search behavior over a bounded first result page.
- A positional structured query initializes Filter; a positional complete JSON string or `--fts TEXT` initializes Search under the CLI's existing classification. Reject `--query-file` together with `--tui`; batch input remains the noninteractive mode.
- `--path`, `--no-recurse`, `--schema`, `--auto-index`, alphabet and snapshot options have the same meanings in both modes. Pass the same configured repository/open operation into the reusable UI; do not reopen it through the old repository-schema loader or build twice.
- Explicit `--limit`/`--offset` constrain the interactive result scope as documented; the TUI's internal page size is a display mechanism, not a silent replacement for these flags. Counts follow the same library semantics.
- Require an interactive terminal for TUI input/output and fail clearly before expensive work when unavailable. Do not silently switch to NDJSON. Do not interleave NDJSON or ordinary diagnostic printing with the terminal frame; show status/errors in the UI and print fatal diagnostics after terminal restoration.
- Use the existing loading screen for build/restore, identifying which operation actually occurs. Restore raw mode, alternate screen, mouse capture and cursor state on quit, startup error, execution failure and supported interruption paths.
- Preserve query generations, cancel superseded work and bound pending results. Stale queries must not replace newer rows, counts, columns or timings. Do not accumulate one unbounded worker per keystroke.
- Reuse the accepted streaming/page/cache mechanisms. Keep viewport/page payloads bounded, load details on demand and avoid repeated full scans while navigating. Do not materialize all matching full JSON documents merely because noninteractive output defaults to all results.
- Compute snippets and field-column projections for needed rows, cache them within bounded page state and reuse the source decoding already available. No parsing, query execution or fallible user operation inside every render frame.
- TUI timing/status reports execution separately from display/projection work where measurable. Query history and UI preferences use the established settings mechanism, scoped so they do not overwrite another collection's state; they never silently enable schema loading, inferred indexes or index persistence.
- Keep explicit export actions and their key bindings. Export all through the prior single-execution streaming contract; export the full records, not only the visible columns.

### 7.6 Test and benchmark frontend

Document `obj` as the standard general-purpose frontend for testing library behavior, inspecting results and collecting reproducible benchmark data. The CLI and TUI must call the same parser, preparation, execution, sorting, evidence and snapshot APIs as library consumers; no CLI-only query evaluator or special fast path for the example corpus.

Use `--query-file` plus NDJSON for repeatable machine-readable runs, and `--tui` for interactive checks of query inputs, regex/case options, field projections, validity and sort behavior. Benchmark search with the noninteractive mode: terminal rendering and human interactions are not stable performance measurements. Keep TUI responsiveness measurements separate.

Provide documented commands/fixtures for the four schema/inference configurations in section 7.4, snapshot build/restore, batch input, FTS/regex and structured queries, dynamic sorting, all-results streaming, malformed records and valid arbitrary JSON. Preserve existing library instrumentation/EXPLAIN facilities where exposed; record actual chosen access paths, candidate/read/parse counts, effective schema/inference policy, corpus and software revision. Do not add a second benchmarking framework or require a new benchmark subcommand.

The phrase “test frontend” does not imply that unsupported multi-collection features now work in this single-collection command. Test supported capabilities here, and keep focused library tests for features outside its scope. It complements, rather than replaces, the unit/integration and performance suites.

## 8. Snapshots, reopening and caches

Use the earlier explicit load/write state machine. Add short-string policy/version, decoded-value representation, string comparison-key identity, coverage codec and inferred capability configuration to the established compatibility mechanism. Bump the applicable snapshot revision when the layout changes.

Include the effective explicit schema, manual indexing choices, sort declarations, prohibited paths and schema-adapter semantics in compatibility. Use content/effective-configuration identity, not only the schema filename or modification time. A changed schema at the same path cannot silently reuse incompatible postings. Add/remove `--schema` under the same load-only versus load-and-write rules as other configuration changes. A relocated but identical schema may reuse a snapshot only when the effective collection and excluded-source inventory also agree. TUI display settings are not index configuration and must not force index rebuilds.

For this supplement, require matching effective inference mode/policy for an ordinary restore. Do not silently enable persisted inferred indexes when `--auto-index` is absent, or report AutoIndex as fulfilled by a snapshot lacking the new capability. Reusing base sections across modes is not required.

| Flags | Compatible current snapshot | Absent, stale, corrupt or policy-incompatible snapshot |
| --- | --- | --- |
| Neither index flag | Fresh in-memory build with requested mode | Same; do not inspect disk snapshots |
| `--write-index` only | Fresh build with requested mode, then atomic save | Same |
| `--load-index` only | Restore matching mode/policy and validate | Startup error with a precise rebuild instruction |
| Both index flags | Restore matching mode/policy under the existing write contract | Rebuild with requested mode, then atomic save |

Permission/read/write failures retain the earlier error treatment. Automatic indexing does not authorize deleting unrelated files or changing the existing inventory reconciliation policy.

On restore, validate canonical paths, comparison-key ordering, record-slot bounds, duplicate membership invariants, coverage sets and completeness claims. Partial indexes remain partial after serialization. A malformed coverage section cannot be interpreted as complete merely because the B-tree section decoded.

Do not rebuild inferred indexes by reparsing the corpus when loading a compatible snapshot. Opening may still enumerate/stat the selected inventory as already specified. A build-to-restore benchmark must separate inventory reconciliation, deserialization and source parsing.

Physical prepared plans must not outlive the indexed generation or capability configuration. Verified membership caching must retain the existing semantic/configuration ownership; the new access path must not publish an incomplete scan as a complete answer. Cancellation and streaming errors preserve the earlier no-partial-cache-publication rule.

## 9. Required correctness evidence

Use independently specified expected results as well as differential comparison with forced scanning. Existing modes must agree with caches disabled and warm, fresh and restored. Do not rely only on tests comparing two paths that share the same new implementation.

### 9.1 Admission and representation

- Lengths 0, 1, 16, 36, 63, 64 and 65 bytes; multi-byte Unicode; escaped JSON; embedded NUL; whitespace-only strings. Include ordinary, braced and URN UUID strings and verify that admission requires no UUID parsing.
- More than 64 distinct short strings, low repetition, and a one-record field, all admitted under sufficient budget.
- High-cardinality short strings do not get rejected by enum thresholds.
- Low-cardinality strings retain their earlier policy without duplicating short-string entries. Values beyond 64 bytes require a separately configured eligible route or manual index; default enum settings do not admit them.
- A late long value, mixed type, or failed enum candidate retains eligible short-string memberships.
- Logical numbers/booleans/null remain separate from their FTS text and from strings with identical spelling.
- Nested fields, unusual property names, root strings, root arrays, mixed arrays and repeated occurrences.
- Explicit prohibitions, reuse of compatible explicit postings, incompatible/custom reader coverage.

### 9.2 Planner and structural behavior

- Selective short-string equality, inclusive/exclusive compatible ranges, absent keys and parameter type changes.
- Short and long values in the same field, and in the same record's array.
- `code > "A"` with the long matching value specified above.
- ANY/default ALL, empty arrays, nulls, missing paths, nested empty ancestors and fallback records.
- A short matching array element paired with a long nonmatching element must not falsely satisfy ALL.
- Negation, inequality, Boolean composition and observable operation errors.
- Descendant paths discovered after empty ancestors, including separate worker batches.
- Collation/case/normalization cases that distinguish the reference semantics from naive byte equality/order.
- Positional selectors and array-expanded indexes without assuming occurrence order from deduplicated postings.

Assert both correct final answers and candidate-superset safety. In a mixed field, counters must prove that unindexed possible matches are either evaluated or excluded by a tested exact proof.

### 9.3 Modes, CLI and persistence

- Disabled has no inference-owned maps/postings and unchanged default behavior.
- ReportOnly does not construct the full high-cardinality B-tree.
- `--auto-index` enables inference plus Boolean/numeric/low-cardinality/short-string admission through the same library policy.
- Every CLI query input mode and index-only operation works; help/version perform no indexing.
- Neither index flag reads/writes snapshots; load-only does not write.
- A snapshot built without AutoIndex is rejected by AutoIndex load-only; both flags rebuild it.
- The reverse mode mismatch is also explicit; omission of `--auto-index` never restores inferred indexes silently.
- Threshold/codec/policy changes, malformed coverage and stale inventories follow the compatibility contract.
- Fresh/restored IDs, counts, ordering and payloads agree; mixed-field coverage survives round trips.
- Boundary resource limits, deterministic rejection, worker-count changes and batch-order changes preserve correctness and report completeness.
- Streaming cancellation, late failure and slow output preserve the earlier completion/cache contracts.

### 9.4 Schema integration

- Load the documented plain schema with no `x-objekte` wrapper, no collection directories and no implicit source-root/identity changes.
- Test all four schema/AutoIndex combinations, explicit false versus unspecified, compatible explicit-index reuse and contradictory requests.
- Prove manual indexes work with AutoIndex disabled and can include string values longer than 64 bytes under their explicit budget contract.
- Cover nested/root/array types, nullability, missing fields, supported local references, unsupported unions/references and existing declared-type failure diagnostics.
- Reject malformed schema/index entries; do not silently ignore a requested manual index or reinterpret unsupported constraints as validated.
- Exclude the selected schema from a recursive source inventory; preserve ordinary JSON discovery when no schema is supplied.
- Schema content change, flag removal/addition, short-string limit change and explicit-index changes invalidate incompatible snapshots even when filenames remain unchanged.
- Show that no network schema fetch, source edit or schema rewrite occurs.

### 9.5 TUI integration

- Render test frames for the existing dark/light theme and narrow/wide terminals, preserving Search/Filter, Aa/regex controls, status, selection and overlays.
- Replay the retained key bindings through the event handler, including focus-sensitive input editing, AltGr, regex/case toggles, sorting, history, preview, record detail and explicit export.
- With no filter/sort, assert exactly `valid`, `id`, `match` even when schema/inference exposes hundreds of fields.
- Assert filter/sort column derivation, order and deduplication for nested Boolean terms, both field operands, root paths, literal dotted keys, metadata, source `id`, reducers and query removal. No schema-based default columns.
- Sort an undeclared field through inline syntax and the existing-style sort action with inference disabled.
- Check match snippets against actual text/regex/value evidence, including JSON escapes, Unicode, arrays and fallback records; no unrelated title fallback or invented witness for negative predicates.
- Assert no link rows, relation fetches, link activation, URL opening or OSC-8 hyperlinks; source relation values still appear as plain data in detail.
- Start queryless TUI; initialize it from positional/FTS input; reject query-file conflict and non-TTY use; verify one repository opening and consistent schema/inference/snapshot options.
- Simulate slow/superseded queries, cancellation, invalid query edits, window resize, terminal failure and cleanup. Stale results must not update columns or timing.
- Count source parses during repeated redraws of an unchanged viewport: redraw alone must not parse records. Verify bounded page/projection buffers and export using the all-results execution path.
- Retain regression tests for the existing full TUI so its schema columns and links are not accidentally removed from other modes.

## 10. Performance verification

Use the same fixed source revision, corpus, worker count, release configuration and query set for comparisons. Run benchmarks sequentially, without competing builds. Retain raw samples and explain whether the filesystem and verified-result caches are cold or warm.

Compare:

1. Disabled mode.
2. AutoIndex with the previous admission policy, when a comparable baseline is available.
3. AutoIndex including this supplement.

Include Musenalm and deterministic fixtures for low-cardinality strings, high-cardinality short identifiers, mostly-short fields with a few long outliers, mixed arrays, long free text, and many unique property paths. The short-identifier fixture must exceed the old 64-value dictionary threshold by a substantial margin and run within a documented sufficient budget.

Include at least 100,000 distinct ordinary 36-byte UUID strings under sufficient documented budgets, plus 63/64/65-byte boundary fields. Test mostly-short fields with values genuinely longer than 64 bytes; earlier 17-byte outliers no longer exercise partial coverage. Where available, compare the previous 16-byte policy with the new 64-byte policy to expose construction/memory differences.

Use the same batch queries through `obj` under no schema/no AutoIndex, schema/manual indexes only, AutoIndex only, and both. Compare only semantically equivalent fixtures, identify deliberately invalid-schema records separately, and record build/restore costs as well as query benefits. Add terminal-specific measurements for startup readiness, cancellation latency, first visible results, viewport projection and redraw; report these separately from core search timings.

Record fresh build duration, decode counts, inference/finalization work, peak process memory, retained inferred bytes, snapshot size/write/restore time, and selective equality/range query time. Include candidate count, unresolved remainder count, source reads/parses and final result count. Query measurements must exclude terminal serialization when evaluating index acceleration.

Acceptance requires:

- No extra full-source parse pass for enabled construction.
- Selective short-string equality demonstrably reads/parses fewer sources than forced scanning, with verified-result caches disabled.
- Compatible range execution uses bounded B-tree traversal rather than scanning every dictionary key; report output-size costs honestly.
- Default Disabled construction avoids a material regression.
- Resource exhaustion remains bounded and visible.
- Mostly-short mixed fields demonstrate the cost of the unresolved remainder; wholly mixed selections are not advertised as fully indexed.

A repeatable roughly 10% regression on unchanged Disabled workloads requires investigation and supervisor disposition. For AutoIndex, report the actual build-time and memory increase and query benefit; do not promise a speedup or fixed millisecond target without measurements. High-cardinality node/posting overhead is a specific required finding, not an incidental footnote.

## 11. Supervisor, worker ownership and commits

### 11.1 Coordination with ongoing assignments

The supervisor reads the current repository instructions and both prerequisite ledgers, identifies active owners and pins a fixed implementation base. If the earlier inference or CLI worker is still active, assign this as an amendment to that owner. Do not create two writers implementing the same APIs, reset their worktrees, or wait for unnecessary duplicate branches.

If new worktrees are required, create them directly under `/home/simon/source`; never under `/tmp`. Suggested integration branch: `integration/short-string-auto-index`; suggested worktree: `/home/simon/source/objekte-short-string-auto-index`. Use separate descriptive worktrees for additional production writers. A read-only reviewer may share the owning worktree.

Before parallel implementation, commit a small contract note containing: inclusive 64-byte threshold, key/comparator choice, coverage representation, resource defaults, report additions, schema mapping/precedence, TUI reuse boundary, derived-column contract, CLI mapping and snapshot-version ownership. Concrete type names must match the integrated code rather than blindly copying conceptual names from this document.

### 11.2 Assignments

| Task | Owner | Completion condition |
| --- | --- | --- |
| AUTO-00 Baseline and contracts | Supervisor | Fixed SHA, active-owner coordination, comparison semantics and coverage design documented. |
| AUTO-01 Admission and construction | Existing inference/core worker | Short strings bypass enum gates; B-tree memberships, bounded builders and observation reports implemented. |
| AUTO-02 Coverage and planner | Core/query worker, or AUTO-01 owner if overlap is substantial | Mixed-field remainder, compatible equality/ranges, quantifiers/errors and candidate-superset tests completed. |
| AUTO-03 Persistence and configuration | Supervisor-designated storage owner | Policy/codec compatibility, coverage validation and restore tests completed. |
| AUTO-04 CLI integration | Existing CLI worker | `--auto-index`, startup mapping, output summary, process tests and CLI documentation completed. |
| AUTO-04A Schema adapter | Schema/core adapter owner coordinated with CLI worker | Shared plain-schema adapter, type/manual-index precedence, reference/error handling, exclusion and compatibility tests completed. |
| AUTO-04B Reusable TUI mode | Existing TUI owner, separate production worktree if concurrent | Same appearance/controls, restricted query-derived columns, accurate evidence, no links, bounded execution and terminal lifecycle completed. |
| AUTO-05 Independent verification | Bounded review/test subagents | Byte-boundary/coverage/negative tests and resource-policy review; blocking findings resolved by owners. |
| AUTO-06 Integration and promotion | Supervisor | All reviewed work merged, combined checks/benchmarks run, development promotion completed. |

Use one writer per shared implementation area. Independent CLI argument work may proceed against frozen core contracts. Serialize overlapping `repository.rs`, posting, planner, schema adapter, TUI application state and `storage.rs` changes; the supervisor owns conflict resolution and final version allocation. Freeze the reusable TUI entry point and query-projection contract before CLI and TUI workers integrate them.

Required review subagent questions:

1. Can partial string coverage omit a match or hide an error for ANY, ALL, NOT, ranges, null, empty selection or a parameter type change?
2. Does a high-cardinality short-string field remain admitted without enum heuristics, while all worker/merge/index allocations remain within the declared budget?
3. Does the CLI map all modes and snapshot combinations correctly, preserving the disabled default and the one-build-per-invocation rule?
4. Do schema declarations and manual index requests use one adapter, preserve explicit prohibitions and avoid unsound type/coverage assumptions?
5. Does the TUI preserve the original key/focus contracts while showing only the requested columns, disabling all links and avoiding source work during redraw?

Reviews return concrete findings and reproduction cases. Production fixes remain with the responsible owner; no unreviewed shared-file changes from review subagents.

### 11.3 Commit sequence

Adapt the exact boundaries to active work while preserving understandable, testable commits:

1. `feat(core): define short-string automatic indexing policy`
2. `feat(core): build ordered postings for short string values`
3. `fix(core): preserve residual coverage for inferred string postings`
4. `feat(storage): persist inferred string coverage and policy`
5. `feat(cli): add automatic indexing option`
6. `test(core): verify short-string indexing across mixed values`
7. `test(cli): verify automatic indexing and snapshot modes`
8. `docs: document short-string indexing costs and command usage`

Additional schema/TUI milestones, integrated before final acceptance:

- `feat(cli): load collection type hints and manual indexes from schema`
- `refactor(tui): share terminal components with the general CLI`
- `feat(tui): derive result columns from active filters and sorting`
- `feat(cli): add interactive mode without link navigation`
- `test(cli): verify schema and interactive configuration parity`
- `docs: define CLI benchmark and interactive test workflows`

### 11.4 Mandatory final integration

The supervisor must complete the combined result, not stop at approved worker branches:

1. Review fixed worker heads and resolve blocking findings.
2. Merge core admission/coverage/planner work, then shared schema/persistence and CLI work, followed by the reusable TUI mode into the agreed integration branch. Where earlier assignments already have an integration branch, coordinate inclusion there instead of duplicating merges.
3. Resolve shared contracts explicitly; do not discard one side of a conflict wholesale.
4. Allocate final snapshot/policy revisions once and run the repository's required formatter, lint, build and relevant unit/integration checks on the merged commit.
5. Run the combined acceptance matrix and sequential performance measurements; rerun affected checks after fixes.
6. Commit a concise implementation/measurement report with revision, corpus, budgets, admission counts, unresolved limitations and raw-sample locations.
7. Promote the validated result to `development/local-file-search-core` through the established non-force merge/PR workflow. Do not target `master` or rewrite shared history.

Routine final integration is part of this work order. Respect actual branch protections; if they block promotion, retain the tested integration result, prepare the concrete PR and identify the blocker. An unmerged PR must not be reported as a completed development merge.

## 12. Supervisor starting instruction

> Apply this supplement to the ongoing optional-indexing and `obj` work. Preserve the schema-free, inference-disabled default. In AutoIndex, admit every supported decoded string value of at most 64 UTF-8 bytes, including UUID strings, without the low-cardinality heuristic, subject to explicit resource limits and opt-outs. Retain coverage and residual evaluation for long or mixed values. Implement `--auto-index`, `--schema FILE` and `--tui` through shared library configuration and the existing TUI components. Keep the same TUI appearance and applicable key bindings, only validity/ID/match plus active filter/sort columns, and no links. Establish `obj` as the reproducible test/benchmark frontend. Coordinate active owners, complete independent reviews, persistence and performance evidence, then merge the validated combined result into the development branch.

## References

- [Optional indexing, broader sorting and streaming specification](https://chatgpt.com/library?file_id=libfile_8f10deace404819196483e37820231c4&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2), especially sections 5 and 8.
- [Original `obj` CLI implementation plan](https://chatgpt.com/library?file_id=libfile_6fbac650b85881918c6550000fbaefed&account_id=89ce9547-be86-4c0f-a24f-189ad5a0a1d2), especially sections 2–4, 8, 11–12 and 14.
- Existing TUI at the inspected `f374101`: [key dispatch and lifecycle](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/tui/src/tui/mod.rs), [query and column state](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/tui/src/tui/app.rs), [rendering](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/tui/src/tui/view/mod.rs), [theme](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/tui/src/tui/view/theme.rs), and [schema adapter](https://github.com/Simon-Martens/objekte/blob/f374101b4839f28199a99677118ac96869203bb0/crates/tui/src/schema.rs).

These plans and the identified TUI/schema sources are the specification basis. The executing supervisor must inspect and pin the actual current implementation before coding; the inspected development revision is a UI/adapter reference and does not assert that the still-running prerequisite changes have already merged.
