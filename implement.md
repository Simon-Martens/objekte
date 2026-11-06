# Local File Search Engine — Core Implementation Specification

This is the complete intended specification for the new program described here, written as if the project were being started from scratch. It defines the required search behavior, logical model, performance priorities, test CLI, and validation. It is not a requirement to rewrite every component: the implementing agent may freely inspect, reuse, adapt, refactor, or replace existing code to achieve this design.

The previous project will be moved into `old/`, with its core at `old/crates/core/`. The existing `examples/` directory remains at the repository root. Develop the new core in `core/` and establish the new workspace around it. Access to the previous implementation is unrestricted; the former restriction to bigram, posting, and sorting code is removed. Architectural examples and suggested internal structure guide implementation without prescribing every module, type name, or algorithm where the required behavior leaves a choice.

Future index mutability must inform the design, but **do not implement update handling in this version**. Later updates will supply complete files, and the engine will derive their records. Implement and persist the file-to-record index now so a future file replacement can identify every record contributed by the previous file revision. Initial construction and restoration may publish immutable index generations; reopening may rebuild changed collections. Section 15 defines the future whole-file boundary without requiring an updater today.

## 1. Objective and scope

Build a new Rust search core for repeated, very fast searches over local files. Discover sources and build the configured in-memory indexes when a repository opens, or restore a compatible snapshot and reconcile changes. Keep this state resident across queries.

The searchable unit is a **record**. A file contains zero, one, or several records. Indexing, filtering, sorting, pagination, and COUNT operate on records. File metadata, byte access, and change detection operate on files.

This is a new implementation, with no compatibility requirement for an existing external API. The previous project is available under `old/`, including `old/crates/core/`; it is not a sibling crate of the new `core/`. Keep `examples/` in place. Implement the Musenalm test CLI and benchmark harness described below, but no public plugin API, network service, or graphical interface.

### Full access to the previous implementation

The specification and the current fixture schemas are the implementation contract. The agent may read anything in `old/crates/core/`, including its query parser/planner, repository architecture, scanners, extension framework, caches, orchestration, indexes, serialization, and tests. The rest of `old/` is also available for context and reuse. There is no index-only access restriction and no requirement to avoid learning from the previous architecture.

Choose the implementation freely within this specification: retain useful algorithms or components, reorganize them, port them into the new build, or implement replacements. Do not preserve obsolete limitations solely because they exist in the old code. Record substantial reuse and important design decisions, preserve applicable license notices, and verify reused behavior against the new contracts. The fixed 150-query workload remains the acceptance workload; previous tests and benchmarks may be studied or reused as supplementary material.

The new program must build and run through its new workspace and core entry points. Make reused components part of an intentional, documented build arrangement; the choice of copying, refactoring, or sharing suitable modules is left to the agent. The user will move the previous project into `old/`; do not perform that move again or overwrite the preserved project. Resolve fixture paths from the repository root, so `examples/musenalm/` remains the test repository rather than becoming `old/examples/musenalm/`.

The logical record model must accommodate JSON, JSONL, Markdown with frontmatter, YAML, TOML, XML, and suitable HTML-derived data through source-specific extraction. These are possible applications of the model, not encodings built into its query planner. Do not implement XML, HTML, or Markdown readers in this assignment.

The three primary index families are:

1. B-tree posting filters for typed values, null, presence, and derived values.
2. Sorting lists using ICU collation for strings and natural typed ordering for other sortable values.
3. Bigram filters over extracted record text, including the collection default named `raw`.

Prioritize repeated-query latency, useful candidate reduction, and reuse of verified results. Measure construction, restoration, and memory as well. Consider later whole-file update cost in architectural decisions, but leave update execution out of this assignment. Do not introduce relevance ranking, fuzzy matching, embeddings, or a general database layer.

Make typed, cost-aware query planning central to the core. Each term must select an appropriate physical representation and compatible access path; Boolean terms must be reordered to apply economical restrictions before expensive work. Source order is not an execution plan. Optimize for total query work using selectivity, coverage, source-access cost, and required output, with inexpensive selective indexes normally evaluated first. Section 12 defines the required planner and its diagnostics.

## 2. Ownership and architectural boundaries

### Recommended ownership

Use `Repository` as the owning handle and execution coordinator. Place collection-specific indexes and record state in independently shareable collection generations. Publish a repository snapshot that references a consistent combination of these generations and file metadata.

| Structure | Owns or references |
|---|---|
| Repository | Root directory, repository options, collection definitions, internal registrations, query space, cache budgets, shared workers, change coordination, current published snapshot |
| Query space | Collection and field name resolution, available search bindings, query preparation, and cross-collection planning; references indexes without duplicating them |
| Repository snapshot | File inventory/revisions and references to published collection generations; an immutable view retained by a running query |
| Collection generation | Collection configuration, file membership, record inventory, slot domain, field/type information, posting filters, bigram filters, sorting lists, coverage, generation number |
| File entry | Repository file identity, path, metadata, recorded modification time, scanner binding, and references to records extracted from that file |
| Record entry | Collection-scoped record identity, compact record slot, file reference, byte range, source revision, and extraction status |

This gives one owner for shared resources without forcing all collection indexes into one mutable structure. A collection update can share unchanged state; queries execute without a repository-wide lock held through scanning or sorting.

Separate index construction and publication from the read-only query view. Immutable published state is compatible with future index mutability: updates may construct a replacement generation or partition privately and publish it, while existing readers retain their captured state. This version may rebuild a whole collection. Do not expose physical buffer addresses or permanent dense-slot positions as public record identity, or require the query planner to know how updates are implemented.

The repository owns caches, with entries namespaced by collection identities, relevant generation numbers, and resolved semantics. A change to an unrelated collection should not invalidate an otherwise independent cached query. Worker pools are repository resources, not separately created for every collection or query.

Capture the selected collection states together when a query starts. A query spanning collections must not acquire arbitrary generations at different points during execution. Updates affecting a file included in several collections must publish the corresponding changes consistently.

### Internal responsibilities

Keep source discovery/framing, logical extraction, index construction, language parsing, planning, execution, result materialization, and state persistence distinct.

The planner sees logical types, predicates, and index capabilities. It does not parse a file format. Index builders consume record slots and extracted values. Scanners understand record bytes and verify the prepared predicates.

The query space owns binding and logical preparation; collection generations expose compatible access paths and compact statistics to the optimizer. The executor accepts candidate restrictions and reports actual cardinalities so later work can adapt. Neither index selection nor predicate ordering belongs inside a format-specific scanner. Scanners receive prepared residual predicates and may short-circuit them without resolving names or rebuilding the query plan.

Use ordinary internal Rust registrations and prepared operations. There is no need for a dynamic plugin loader, stable ABI, or public SDK. Keep dynamic dispatch outside per-character/per-value loops where possible; resolve registrations once during collection/query preparation.

## 3. Repository, collections, files, and record ranges

### Repository and collection definitions

A repository represents one root directory and its configured query space. A collection selects files, normally beneath a directory or subdirectory, with explicit recursion, inclusion, exclusion, hidden-file, and symlink rules.

A collection carries an optional schema or type hints, record-ID extraction policy, source reader/scanner selection, text/value extraction definitions, field-to-index bindings, and sorting defaults. A schema is helpful but not required for every collection.

Use native filesystem path types. Do not assume paths are UTF-8, lowercase them indiscriminately, or include index snapshots and temporary output in the source selection.

### File inventory

Keep file identity separate from record identity. File metadata includes repository and collection association, path, byte size, created time where available, and modified time. Unsupported created time remains unavailable; never substitute Unix metadata-change time or modification time and label it creation time.

Use file modification time, at the available filesystem precision, as the initial revision check. Store byte size for metadata and range validation, and use a size difference as an additional cheap change signal. No mandatory content hashing is required in this version.

This detects ordinary changes but cannot establish content identity when an edit preserves modification time and size. Document that limitation once; do not add per-query collection hashing to compensate.

### Reserved metadata fields

All query-visible record, file, collection, and repository metadata uses a leading `@` namespace. User data fields named `id`, `size`, `created`, or `collection` remain ordinary logical fields and must not be shadowed.

| Metadata field | Type and meaning |
|---|---|
| `@record.id` | Collection-scoped record ID |
| `@record.byte_start`, `@record.byte_end` | Numeric byte-range bounds |
| `@record.byte_length` | End minus start |
| `@file.id` | Repository file identity |
| `@file.path` | Repository-relative source path |
| `@file.size` | Numeric source-file byte size |
| `@file.created` | Nullable datetime; null if creation time is unavailable |
| `@file.modified` | Datetime corresponding to the recorded filesystem modification time |
| `@collection.name` | Collection name |
| `@repository.root` | Repository root path |

Metadata selectors are a distinct kind of resolved field path, populated from the inventory. Apply their typed comparisons, sorting, presence/null operations, and optional indexes without parsing record contents. One file's metadata can be shared by many records; matching it still returns/counts records.

Keep native paths internally. Expose a documented portable path-string representation for queries, using forward-slash separators for `@file.path` and a reversible representation for otherwise non-UTF-8 names. Never mistake a display string for a native file handle.

Only an unquoted leading `@` selects metadata. Quoted logical names such as `["@file"].size` and `data.gnd["@context"]` address user data. Reject unknown metadata names rather than silently treating them as missing user fields.

Keep returned metadata in prefixed containers such as `@record`, `@file`, `@collection`, and `@matches`, separate from the selected record payload. Do not merge unprefixed metadata into the logical record object. Ordinary query options such as limit, offset, and COUNT are request controls, not record fields.

The default raw text extractor excludes metadata. Searching metadata text requires an explicitly selected metadata field or a FILTER predicate, planned against inventory/index data.

### Byte-range record mapping

Each record has a source reference consisting of:

- File identity.
- Start byte offset, inclusive.
- End byte offset, exclusive.
- Recorded file modification time and collection generation.

Use checked 64-bit byte offsets. Validate ordering and file bounds. Offsets refer to original source bytes, not Unicode character positions, decoded strings, or normalized text.

The selected scanner must be able to parse the byte range **standalone**, using its prepared configuration and collection type hints. It must not need to parse earlier records or reconstruct the entire file to interpret the selected record.

The source reader discovers record boundaries during indexing. Those boundaries exclude unrelated separators and enclosing syntax as necessary for standalone parsing. A single-record file may use the whole file range. A line-delimited format records an independently parseable range for each record.

Formats whose records depend on preceding declarations or other source context require a different framing/application strategy before admission to this initial standalone-range contract. Do not silently turn every candidate lookup into a full-file parse.

Maintain both mappings: record slot to file/range, and file to its record slots. Use compact arrays and offset tables where suitable. A file modification invalidates all of that file's old record ranges, even when some byte offsets happen to be unchanged.

### File-to-record index: required in this version

Maintain an explicit `FileRecordIndex` in each collection generation, with the logical mapping `FileId -> RecordSlotSet`. Its entries enumerate every record extracted from that file in that collection. A compact flat slot array with a per-file offset/count table is suitable; a file entry pointing into a contiguous range is sufficient only while construction guarantees that range contains exactly that file's records. Do not require scanning the entire record inventory to find the records of one file.

The reverse record-to-file/range mapping and the file-to-record index must agree. Each indexed record belongs to exactly one source file within its collection. Files containing zero records have an empty entry; an extraction failure remains a diagnostic and must not be mistaken for a successfully empty file. A file used by multiple collections has a separate record set in each collection's generation.

Build both directions during the existing framing/extraction pass, without a second source parse. Persist and restore the file-to-record index with its file revisions and generation-specific record slots; validate ownership and slot bounds. Record slots and byte ranges from a different generation are never interchangeable.

This index is required now even though updating is deferred. Later, a complete replacement file can have different record boundaries, counts, and IDs. The stored mapping identifies the old record set without requiring the overwritten source bytes; the new file is framed and extracted afresh. It is not a requirement to retain old record contents or every record's reverse list of posting/bigram contributions.

### Record identity

Each record has an ID unique within its collection. Repository-wide identity is the pair of collection ID and record ID. Compact internal record slots, normally `u32`, are separate from these identities and belong to a particular generation; validate capacity and overflow.

Prefer an ID extracted from the record when configured. Detect duplicate IDs and report them; never silently replace one record's postings with another's.

When no intrinsic ID exists, generate an ID from file identity, source revision, and record start offset. This generated identity is revision-bound and is not promised to survive insertion, movement, or editing of records. Its namespace must not collide with explicitly extracted IDs.

All index families for a collection use the same record-slot domain. Reusing a supplied filter implementation must not accidentally preserve file-level IDs as if they were record IDs.

## 4. Logical record model

The core owns a format-independent typed value model, similar to JSON but with explicit temporal types. A record's logical data is a nested object of named fields.

| Type | Required meaning and suggested representation |
|---|---|
| String | Unicode text, normally UTF-8 |
| Number | Integers represented without precision loss, plus a documented finite floating-point representation where needed |
| Bool | True or false |
| Date | Calendar date without time or timezone; validated calendar value or day count |
| Datetime | An instant with subsecond precision; normalize to UTC seconds plus a fractional component for comparison |
| Unixtime | Signed integer seconds since the Unix epoch; distinct from an ordinary number and from a datetime value |
| Object | Named fields containing logical values, including nested objects |
| Array | Ordered elements of one logical element type; the element type may be Object or another supported type |
| Null | Explicit absence of a value, where permitted by nullability |

Date, datetime, and unixtime are mandatory supported types. Preserve their distinctions through parsing, index keys, comparisons, and returned logical data. A date is not implicitly midnight in the machine's timezone. An offset-free datetime requires an explicitly configured interpretation or an error. Validate temporal ranges and precision; do not compare timestamps as arbitrary formatted strings.

Use a tested time library for calendar and timestamp parsing. Parse date/datetime query strings according to the resolved field type. Parse unixtime query values as signed seconds. Do not infer dates from every string that resembles a date.

For numbers, use signed/unsigned integer variants and finite floating point, with one documented exact comparison/canonicalization policy. Do not round large integers through floating point. Reject unsupported values or out-of-range literals rather than indexing a different value. Equal values must produce equal posting keys.

Field definitions carry name/path, logical type, required/optional status, nullability, and array element information. A type hint can direct extraction and query-literal parsing; a value that violates an enforced type is an extraction/type error, not an empty string or a missing field.

Homogeneous arrays are required. Distinct object shapes can still share the Object element type. Nullable elements are permitted only when element nullability allows them; null is not an excuse to mix unrelated element types. An empty array takes its element type from hints where available, otherwise it remains an empty array with unresolved element type until an operation needs that information.

Preserve object/array structure and element identity. Intern field names and compile field paths once. A compact ordered field vector or suitable map is sufficient for objects; do not force an allocated general object tree when borrowed values or streaming extraction suffice.

Distinguish missing fields, explicit null, empty strings, empty arrays, invalid values, and parse errors. Include presence and type information independently of searchable text.

JSON-compatible output is a serialization of this model, not the core's type definition. Date and datetime may serialize as canonical strings and unixtime as a number; retain their types in the logical result/schema. There is no requirement to round-trip type distinctions through an unannotated JSON value alone.

## 5. Internal readers, extraction, and search bindings

Register internal source readers/scanners, value extractors, text extractors, and sorting definitions. Collections associate these registrations with fields. The engine owns discovery, scheduling, storage, planning, set operations, caching, and publication.

A source reader identifies record ranges and exposes each record's values. Its associated scanner can independently parse one recorded range and evaluate prepared work. Use small reference readers in the harness to demonstrate single-record files and multiple independent records per file. Keep a particular format parser outside generic indexing/planning components.

### Default and custom search bindings

A field can have several named search bindings. Each binding defines one coherent operation path:

1. Accepted field types and operators.
2. Parsing and preparation of the filter's right-hand value.
3. Extraction/transformation of the record's field values or text.
4. Associated posting tree and/or bigram filter.
5. Scanner preparation and exact verification using the same semantics.
6. Coverage and semantic/configuration version.

Thus `field:custom` selects the registered value parser, index path, and scanner together. It does not merely rename a bitmap. The same bound operation must work with an index, partial index coverage, or scanning alone.

Resolve built-in modifiers first and reserve their names. Custom names must be unique for a field within the collection; reject collisions and unknown names. Resolve everything before execution. Do not perform string registry lookups for every record.

A field without an explicit custom modifier uses its configured default binding. An FTS query without a selected field or binding uses the collection's default `raw` text target. Selected FTS fields can each carry a binding name; resolve that through the same registration used by FILTER.

A known binding with no built index remains executable through its scanner. An unknown binding is a query error. Falling back to a different representation is not permitted.

Allow only defined modifier compositions. For example, an array quantifier may apply a string transformation to each element. Reject incompatible or ambiguous chains rather than guessing an order. Version the resolved operation so caches and snapshots cannot mix old parsing rules with new scanners.

## 6. Index construction and parsing budget

During startup, enumerate selected files once into the authoritative inventory. Build into private state and publish only when inventories, record mappings, coverage, and indexes agree.

For each source file:

1. Establish its revision and discover independent record ranges.
2. Parse each record once.
3. From that same parsed record, extract identity, logical fields, raw/custom text, posting values, presence/null state, array lengths, and sorting keys.
4. Feed all relevant builders.
5. Release temporary state or reuse worker buffers.

Record-boundary discovery may inspect framing bytes, but must not first build a complete parsed object and then parse that object again for each index. When the reader already parsed a record to discover its boundary, reuse that parse.

At query time, parse each candidate record at most once to confirm/reject the combined prepared query. Reuse that parsed state for matched-field evidence and logical result output. Do not add another parse solely because a verified record must be returned.

An exact posting match needs no verification parse. It needs at most one later parse if logical output or otherwise unavailable match evidence is requested. Raw-byte output needs a range read; locator output may require no source read at all.

Group candidate work by source file, retain/reuse a file handle within the file work unit, and order/coalesce nearby range reads. Choose bounded range reads for sparse candidates and sequential/coalesced reads for dense candidates. Do not read a large multi-record file from the beginning once per candidate.

These are per-execution guarantees: indexing parses once per record; a subsequent uncached verification parses that candidate once. They do not require retaining all decoded records permanently between executions.

Keep result data only for the selected page or possible page winners. Choose an order-aware strategy or bounded winner storage so verification and return-data production share one parse without retaining the whole corpus. Account for the memory cost of deep offsets/top-k selection. Exact COUNT-only and locator-only operations do not need record payload buffers.

Use bounded parallel work, reusable per-worker input/decoder/normalization buffers, and local builder batches. Avoid a lock per value, a heap allocation per character, or an entire-corpus staging buffer.

## 7. B-tree posting filters

Use an ordered B-tree mapping typed keys to sets of record slots. Rust's `BTreeMap` is the default implementation choice. A posting set may be a compact sorted vector, Roaring bitmap, or an existing appropriate compact bitmap. The B-tree and the posting-set representation solve different problems.

For each indexed field/binding, include tagged special entries:

- **Presence:** every record where the field exists, including explicit null and empty values.
- **Null:** every record where the field's value is explicitly null.
- **Value:** the ordinary typed keys and their record sets.

Tags are internal enum/discriminant values, not magic strings that could collide with user data. Numeric/date range traversal must exclude special entries. Additional type or array-cardinality entries may be maintained as needed.

Deduplicate a record's repeated array values in membership postings, but retain original array length separately. A null element inside an array is distinct from the array field itself being null; keep element-null and field-null information separate.

Use B-tree equality lookup for scalar equality and range iteration for numeric/temporal predicates. String keys use the binding's equality semantics. ICU collation equivalence must not silently change exact string equality.

Presence and null predicates can be answered from special entries. Absence is the eligible record domain minus a complete presence set. Index array length as a derived typed value where configured.

Maintain complete/partial coverage explicitly. A missing key proves no match only in covered records. Scan eligible uncovered records. Do not label a posting result exact if nested-element correlation, quantification, or an unsupported transformation still needs verification.

An ordinary element posting can prove existential membership. Universal conditions require proving that no element violates the predicate, with appropriate type/null/empty-array information. If existing indexes cannot establish this, use the scanner.

## 8. Default raw and custom bigram filters

### Repository-wide bigram alphabet

Add a repository option `bigram_alphabet` with exactly two values: `ascii` and `unicode`. This is a repository setting, inherited by all collections and their raw/custom bigram filters. Do not build both complete variants by default or introduce per-field alphabet overrides in this version.

Reuse the original ASCII and Unicode bigram implementations, including their character/pair encoding, supported normal/skip variants, compact storage, and serialization conventions. Preserve the original default where it is defined by the supplied bigram component; record the effective setting explicitly in configuration and benchmark output. Do not assume that a new Unicode algorithm is required.

Alphabet choice is a filtering/storage strategy. It must not change the query's exact text semantics, ICU sorting, case-folding binding, or returned matches. ASCII mode must conservatively scan when a necessary condition cannot be represented safely; non-ASCII data must not disappear. Unicode mode uses the original compact observed-pair representation, not allocation of all possible Unicode pairs.

Record the alphabet and implementation/configuration version in snapshots, prepared plans, and relevant cache identities. A setting change invalidates/rebuilds affected bigram indexes and dependent plans/results. Reject or rebuild a snapshot with the wrong alphabet; do not reinterpret its columns.

Run the same correctness cases and Musenalm workload under both settings and compare verified record results. Report their query, construction, restoration, index-size, and memory differences separately.

Every collection has a default bigram filter named `raw`, operating on **extracted text of each record** and producing record-slot candidates.

Raw means the unmodified extracted text representation. It does not mean serialized file bytes. A source reader decodes values; raw text preserves their case, punctuation, and whitespace. The default text extractor selects configured textual fields, normally string leaves, and retains field and segment boundaries.

Separate fields, separate array values, and separate records must not create synthetic substring matches across boundaries. A single aggregate filter may have cross-field false positives, but verification must reject them. Never construct text spanning adjacent records in a multi-record file.

Other logical values participate in FTS only when a text extractor explicitly defines their textual representation. Do not silently turn every number, timestamp, or boolean into a string using an unspecified formatting rule.

Custom filters use their bound extraction/transformation and their corresponding scanner. Index construction, query-value parsing, and verification must agree.

Reuse the supplied bigram storage, normal/skip variants, serialization, and allocation optimizations where suitable. Preserve portable fallbacks for CPU-specific acceleration. Adapt input IDs to record slots rather than carrying over a file-only domain.

Required invariants:

- Bigram hits are candidates, not confirmed matches.
- Candidate selection must never exclude a real match.
- Empty/short queries and unsupported alphabet coverage require conservative fallback.
- Allocate observed Unicode pairs or the supplied compact structure, not a complete Unicode-pair matrix.
- Use normal and skip constraints according to selectivity and cost; fixed ordering is not an architectural requirement.
- Never complement an approximate positive candidate set to implement a negative predicate.
- Preserve normalization/gram state across buffer chunks within a segment and reset at actual segment boundaries.
- Preserve complete raw coverage when combining an aggregate index with compatible field partitions.

Do not silently omit text from raw coverage just because a field also has equality postings. Posting lookup is not automatically a complete substring-search replacement.

If regex matching is later exposed, use parsed expressions to derive only necessary conditions; otherwise scan. Do not expand the current syntax merely to add this feature.

## 9. ICU sorting lists

String sorting uses ICU collation from the first implementation. Prefer ICU4X's Rust collator and generated sort keys. Store a collection default locale and collation options, with explicit field overrides where required.

A sorting definition includes the field/binding, locale, collation strength, numeric-ordering and other selected options, null/missing placement, and a deterministic tie-breaker. Persist the resolved settings and ICU/Unicode/CLDR version identity.

For each configured sortable field maintain:

- A compact record-to-sort-value/key column.
- An ordered record-slot sequence for its configured ordering.
- Null/missing state.
- A stable collection/record-ID tie-breaker.

Generate ICU keys during the existing extraction/index-building pass; reuse keys for repeated values where beneficial. Sort by these keys during queries instead of repeatedly running expensive collation conversion.

Numbers, dates, datetimes, and unixtime use their typed natural order, not ICU and not formatted-string ordering. Arrays require a configured scalar reduction, such as minimum or maximum, or are not sortable.

Choose between sorting a small exact match set using stored keys and traversing a precomputed order while testing membership for a broad set. For multiple fields, use a composite comparator over existing columns; persist additional composite lists only for configured useful combinations.

Define descending order carefully: do not accidentally reverse the required stable tie-breaker or change null placement merely by reversing an ascending vector. Without SORT BY, use deterministic collection/record identity order.

Cross-collection sorting must use compatible types and one compatible collation definition. Resolve that before execution; reject incompatible definitions or prepare an explicit common ordering, never compare incompatible ICU key bytes.

Changing collation options or ICU data invalidates affected keys/lists and ordered-result caches. ICU key generation has startup/memory cost; measure it. Precomputation is justified by repeated comparisons, not assumed to be free.

## 10. FTS queries

FTS is **substring matching** over extracted record text. It is not whole-value equality, token-only search, stemming, fuzzy matching, or relevance ranking.

An FTS request supplies a literal string, collection scope, optional selected fields, and optional binding selections. Without overrides it searches each collection's `raw` target.

The complete supplied string is the substring, including spaces and punctuation. The text `local history` means that contiguous substring in one text segment; it is not implicitly split into two independent terms. The core FTS request does not parse FILTER operators out of the search string.

Raw FTS is case-sensitive. A selected case-insensitive or other custom binding transforms the query and data consistently. Such selection is explicit; raw FTS must not quietly discard punctuation or normalize whitespace.

An explicit empty substring matches an existing string segment, including an empty string. A missing FTS component means no FTS condition. Do not confuse those cases.

The planner may combine string posting trees and bigram filters:

- A posting entry whose complete stored string satisfies the substring can prove membership for that field.
- Finding an equal posting key proves a subset of substring matches; it does not prove that no longer string contains the same query.
- Checking all relevant dictionary keys for substring matches can be exact, but its cost depends on the number of distinct values.
- A B-tree prefix range is useful for prefixes, not for arbitrary interior substrings.
- Remaining bigram candidates require scanner verification against the selected record text.

Union and deduplicate these paths in the same record domain. Use field provenance from indexes where available; otherwise collect it during verification.

## 11. FILTER queries and modifiers

FILTER is a separate structured expression over logical fields. Its surface follows the relevant PocketBase comparison and quantifier conventions, without importing authentication rules, request macros, relation APIs, or SQL storage.

A term is a field path with optional modifiers, an operator, and a typed literal. Combine terms with `&&` and `||`; parentheses override precedence. `&&` binds more tightly than `||`. Prefix `!` may negate a parenthesized expression. Quoted strings support both quote styles and documented escaping.

Use canonical `>=` and `<=`, and `?!=`; reject transposed spellings such as `=>`, `=<`, and `?=!`.

Operators:

- `=`, `!=`: equality and inequality.
- `>`, `>=`, `<`, `<=`: ordered comparison.
- `~`, `!~`: contains/pattern comparison and its inverse.
- `?=`, `?!=`, `?>`, `?>=`, `?<`, `?<=`, `?~`, `?!~`: existential comparison over array/multi-valued paths.

For scalar strings, `~` without an explicit wildcard means substring containment. Support PocketBase-style percent wildcards for patterns; specify wildcard escaping and any additional wildcard characters in the grammar. Candidate planning must use only mandatory literal pieces. FTS always remains literal, even if its string contains percent characters.

### Array quantification

For a multi-valued field/path, unprefixed scalar comparisons apply to all selected elements. A question-mark-prefixed comparison requires at least one matching element. `:each` explicitly selects the all-elements interpretation.

The predicate is inside the quantifier: `values ?!= "a"` is true for an array containing both "a" and "b", because "b" differs. It does not mean that "a" is absent. Use a negated existential equality to express no matching element.

An existential condition is false for an empty array. A universal condition is true for an existing empty array; require a positive `:length` condition when nonempty is intended. Missing/null arrays do not acquire universal truth merely because they supply no values.

Paths through arrays of objects retain element association. Separate existential terms may have different witnesses. Do not claim that `people.name ?= "Ada" && people.active ?= true` proves one person has both properties. A same-element compound scope requires an explicit language construct; reject requests for unsupported correlated syntax rather than flattening their meaning.

### Built-in modifiers

| Modifier | Meaning |
|---|---|
| `:lower` | A Unicode case-insensitive string binding: apply one documented locale-independent case-folding transformation to both record values and query literals; preserve punctuation and whitespace |
| `:length` | Array cardinality, preserving duplicates and counting nullable elements |
| `:each` | Apply the following comparison to every selected array element |
| `:null` | Boolean: whether the field is explicitly null |
| `:exists` | Boolean: whether the field is present |
| `:custom` | A registered field-specific query parser, index path, and scanner binding |

For `:lower`, use an established Unicode case-mapping implementation and version its data. Its Unicode behavior is defined by this engine; do not inherit an accidental ASCII-only implementation. A folded posting/bigram index is usable only when built with the same transformation.

Missing gives `:exists = false` and `:null = false`. Explicit null gives both presence and null true. Thus `field:null = false` alone can include missing fields; combine it with `field:exists = true` to require a present non-null value.

Ordinary non-null comparisons require compatible present values. Missing fields do not match `!=` automatically. Explicit `= null` matches null; explicit `!= null` requires a present non-null value. Type errors and extraction errors are not ordinary missing values. Apply these rules consistently inside array quantifiers.

Typed query literals are parsed once using field hints and the selected binding. Dates/datetimes use quoted representations, unixtime uses numeric seconds, and booleans/null have literal tokens. Hints must not cause lossy conversion.

Examples:

```text
title ~ "archive" && year >= 1900
title:lower ~ "HEIDELBERG"
contributors ?= "person-17"
contributors ?!= "person-17"
contributors:length > 0 && contributors:each != "blocked"
subtitle:exists = true && subtitle:null = false
published >= "2026-01-01"
updated >= "2026-01-01T00:00:00Z"
created_seconds >= 0
address.city = "Heidelberg"
title:custom ~ "prepared by the registered value parser"
(kind = "letter" || kind = "note") && !(archived = true)
```

Produce a concise formal grammar and truth-table tests before optimizing the parser. Keep source spans for errors. Custom literal parsers consume an already delimited literal; they do not take control of the surrounding Boolean grammar.

## 12. Query optimization and shared execution

**Query optimization is a primary architectural requirement and an acceptance criterion.** Do not execute predicates in their written order by default. For every term, resolve its logical type, enumerate compatible access paths, choose the least expensive suitable operation, and order Boolean evaluation to reduce subsequent work. Implement this before final performance refinement.

### Separate inputs, one bound query

FTS and FILTER are distinct input components that lower into one internal logical plan. An internal query request contains collection scope, optional FTS, optional FILTER, sort specification, limit, offset, COUNT request, return mode, and match-evidence options.

If FTS and FILTER are both supplied, the record must satisfy both. Optimize them together: a selective FILTER posting can restrict FTS verification, and selective FTS bigrams can restrict a FILTER scan. The internal expression model may compose FTS and FILTER leaves with explicit Boolean nodes, but do not invent an ambiguous combined text grammar. A request with neither predicate is an explicit match-all request, subject to its normal scope and pagination.

Resolve and validate all field paths, logical types, operators, modifiers, custom literal parsers, text targets, and requested sort definitions before execution. A branch eliminated during execution must not hide an unknown field binding, invalid modifier, or malformed literal. Prepare literals and scanner operations once; do not parse query values or resolve names per record.

Choose a physical representation appropriate to the resolved type: typed numeric or temporal keys for ordered comparisons, Boolean/value postings for equality, presence/null sets for those predicates, and compatible text operations for substring matching. **Optimization must never change the logical type or comparison semantics.** A numeric-looking string remains a string unless its schema or explicitly selected binding says otherwise. Do not convert precise integers to floating point to obtain a faster comparison, substitute ICU equivalence for string equality, or choose a different custom transformation.

Prepare collection-specific plans when collections have different types, registrations, index coverage, or selectivity. Do not force every collection through the same access path merely because the field name is the same.

### Internal planning structures

Use small internal structures with responsibilities equivalent to these; exact Rust declarations remain an implementation choice:

| Structure | Required information |
|---|---|
| Bound predicate | Resolved field/metadata selector, logical type, operator and quantifier, prepared literal, binding identity/version, exact evaluation semantics |
| Access path | Compatible index or evaluator, record domain and coverage, exact/candidate result contract, estimated work and cardinality, required residual verification |
| Plan node | Boolean/set operation, selected access path, child execution order, input restriction, output exactness and remaining predicates |
| Index statistics | Generation-bound counts, posting cardinalities, useful range summaries, bigram frequency information, and source-file distribution estimates |
| Execution statistics | Actual input/output cardinalities, candidates verified, file access, parse counts, cache outcomes, and optional phase timings |

Keep logical predicates independent of their physical plans. A bitmap intersection is a physical operation; it must retain which predicates it proves. Replacing an access path must not lose required verification or change the predicate's meaning.

### Enumerate access paths for every term

Consider every applicable path that can improve the query. Do not attach one permanent execution strategy to a field or operator.

| Available path | Suitable use and required qualification |
|---|---|
| Complete verified-result cache | Reuse exact membership or ordering when the predicate, scope, generation, and semantics match |
| Record identity or metadata inventory | Resolve IDs and evaluate prefixed metadata without opening source files |
| Presence, null, type, or derived-value posting | Answer existence, explicit null, and configured array-length predicates directly |
| Typed B-tree lookup | Exact equality or a small set of compatible keys |
| Typed B-tree range | Numeric, date, datetime, unixtime, or other explicitly supported ordered comparisons |
| Existing typed column or compatible ordered list | Evaluate a restricted set or obtain range/order information when its stored keys preserve the predicate's semantics |
| Compatible bigram filter | Reduce substring candidates using the selected binding, field scope, alphabet, and normal/skip-bigram capabilities |
| Posting dictionary evaluation | Test distinct stored values and union their postings when complete values and matching semantics are available and the estimated work is favorable |
| Restricted scanner | Verify the remaining predicates on candidate records, or scan eligible uncovered records when an index cannot answer them |

An equality posting is not the complete answer to substring FTS: longer values may also contain the query. A raw text filter is usable for a field predicate only when its extraction provides a sound superset of that field's possible matches. A case-sensitive raw filter cannot reject matches of a folded custom predicate without a proven compatibility rule.

Track exactness, coverage, binding/version, record domain, and estimated cardinality for every path. An unavailable index means scan the eligible records, not zero matches. Partial coverage normally creates a covered index branch and an uncovered scan branch. An empty lookup proves an empty term only over complete coverage.

### Cost and selectivity

Prefer inexpensive index operations early when they reduce total work. Do not hard-code a universal order such as all postings before all bigrams before all scans. A large range union or dictionary traversal can cost more than evaluating the same predicate on a few surviving records.

Estimate the work still needed to complete the requested query:

`lookup/filter work + set operations + surviving verification + source access/decoding + required ordering/materialization`

Include both the operation's cost and its expected reduction of the current eligible set. Estimate source work in files and bytes as well as records: 1,000 candidates in one file and 1,000 candidates in separate files can require different access plans. Respect COUNT, payload mode, and evidence requirements when comparing alternatives.

Maintain inexpensive statistics during index construction and updates, and persist them with their generation. Include record/coverage counts, posting cardinalities, distinct-value counts, presence/null counts, typed minima/maxima or coarse range summaries, and useful bigram cardinalities. Reuse existing index statistics rather than duplicating large structures. Do not traverse an entire dictionary, parse sources, or perform a second corpus pass merely to estimate a query.

A bounded greedy planner with explicit cost estimates and deterministic tie-breaking is sufficient initially. Avoid exhaustive enumeration, learned models, or expensive per-query histogram construction. Document heuristic assumptions; estimates affect speed, never correctness. Treat unknown estimates conservatively and use observed sizes once operations have run.

Within a bigram filter, process useful rare grams first and stop when the candidate set becomes empty. Include compatible skip grams according to the supplied implementation's semantics. Do not compute every possible intermediate bitmap if a cheaper restriction already decides the operation.

Choose set representations according to cardinality and available kernels: small sorted record-slot vectors for sparse results, compact bitmaps for broad sets, and streaming intersections where useful. Reuse borrowed posting sets and scratch buffers. Avoid cloning a large posting or materializing the entire collection domain just to intersect it with a tiny candidate set. Representation conversion must have a plausible measured benefit.

### AND: reduce candidates before expensive work

Flatten associative AND nodes where semantics permit, regardless of parentheses or source order. Select an economical candidate-producing operation, then order remaining restrictions by expected total reduction and cost. Intersect smaller or more selective sets first when that reduces intermediate work.

Run cheap exact lookups and compatible bigram restrictions before opening files whenever that reduces source work. Push the current candidate restriction into subsequent index operations or scanners; do not independently scan the full collection for every conjunct. Mark which conjuncts are already proven and evaluate only unresolved predicates for each record.

If a complete exact conjunct yields no matches, stop without opening sources for the other conjuncts. Conversely, an exact hit on one conjunct does not prove the others.

For example, if `description ~ 'Vienna' && category = 'poetry'` has a cheap category posting containing 30 records, the planner should normally obtain those records first even though the substring term is written first. It can then compare restricted bigram work with scanning those 30 records. If the category posting contains almost every record but a rare substring produces five candidates cheaply, the substring filter may be the better first restriction. These are illustrative estimates, not fixture measurements or fixed ordering rules.

Do not always materialize every available index result. After strong candidate reduction, a direct typed comparison or scanner over the survivors can be cheaper than another global range union. If the record must already be decoded for another unresolved predicate, account for the low incremental cost of additional evaluations on that decoded record.

### OR, negation, arrays, and correctness

For OR, union branch results and deduplicate record IDs. Schedule branches using their expected cost and acceptance rate. Already proven matching records can be excluded from later expensive membership evaluation, subject to requested field evidence. Approximate candidates are not proven matches and cannot exclude records from another branch. A branch without an index must still search its remaining eligible domain.

Do not move predicates across AND/OR boundaries in a way that changes meaning. Avoid unrestricted distribution into disjunctive or conjunctive normal form; exponential plan growth is unacceptable.

Complement only exact results over a known eligible domain. Never complement a bigram candidate bitmap as if it represented exact positive matches. Negated approximate predicates require sufficient verification and correct handling of uncovered records, missing fields, nulls, and types.

Preserve array quantifier semantics and element correlation. In particular, `?!=` remains “at least one element differs”; it is not the complement of `?=`. Separate existential predicates may have different witnessing elements. Do not merge them into one per-element range or declare them contradictory unless the logical semantics justify that rewrite.

Allow constant folding, duplicate elimination, and range tightening only when type, coverage, null/missing rules, quantification, and binding semantics permit them. Custom prepared predicates must be deterministic for a captured source revision; reordering cannot depend on callback side effects. Preserve the specified failure/diagnostic behavior for records actually requiring source access; never treat an evaluation failure as proof of a non-match.

### Adaptive execution and residual scanning

After an index operation, use actual candidate cardinality to reconsider the remaining path/order. An unexpectedly small result may justify skipping a costly planned dictionary or range traversal. Keep adjustment at plan-node or batch boundaries with bounded overhead, not a full planner invocation per record.

Group unresolved work by source file and batch requested record ranges. Decode each necessary record at most once, reuse its extracted values, and evaluate all remaining predicates against that state. Within a scanned record, evaluate cheap/high-rejection conditions first for AND and cheap/high-acceptance conditions first for OR. Preserve required matched-field evidence, including evidence that short-circuit membership evaluation alone would not collect.

This does not require dynamic scheduling for every field access. Prepare a useful residual order per batch and adapt only where the expected saving exceeds the scheduling cost. Share physical source access between branches while preserving their logical independence.

### Sorting, COUNT, pagination, and reuse

COUNT returns the total number of distinct **matching records**, before limit and offset. Two matching records from one file count as two. One record found through several fields/indexes counts once. There is no file-count interpretation of COUNT.

SORT BY establishes the final record order; OFFSET and LIMIT select a page after that order. These are query options shared by FTS and FILTER, not part of either predicate's grammar. A harness may render them as `SORT BY`, `LIMIT`, and `OFFSET` clauses without mixing predicate parsers.

Compare filtering a selective candidate set and ordering the survivors with traversing a precomputed sorting list and testing membership. Prefer stored typed/ICU sort keys over parsing sources to sort. For page-only requests, early termination is permitted only when the required global order and preceding offset are established. Exact COUNT can prevent early termination even when the page is already full.

Reuse prepared predicates/plans across repeated requests with compatible bindings and generations. Keep verified membership and ordered-result reuse independent of source-content caching. Reordered equivalent predicates may share a canonical logical cache key when semantics permit; physical execution order must not define the query's meaning.

Planning and adaptation overhead are part of query latency. Report prepared-plan hits separately from verified-result hits so a warmed result cache does not conceal an inefficient uncached execution plan. An index-free scanner/reference path remains available for correctness comparisons.

### Explainability

Provide a CLI diagnostic mode equivalent to `EXPLAIN` and an execution mode equivalent to `EXPLAIN ANALYZE`; these are internal/test capabilities, not a public service API. For every term, show the resolved type and binding, relevant compatible paths, selected path and reason, execution order, coverage, exact/candidate status, estimated input/output cardinality, and required residual work. Include meaningful rejected alternatives without enumerating combinations that were never considered.

Execution diagnostics add actual cardinalities, source-file/byte access, record parses, residual evaluations, cache outcomes, and adaptive changes. Distinguish index-proven matches from approximate candidates. Retain original term positions for readable explanations even after reordering.

Capture representative plans for every benchmark case/profile, plus any materially different adaptive execution, in the saved report data. Keep expensive tracing outside primary latency samples and label diagnostic timings separately; lightweight work counters and their overhead must be consistent across comparisons. A report must make it possible to explain why a query opened files and why a different available index was not chosen.



## 13. Candidates, verification, and result modes

A candidate is a record whose complete query has not yet been proven. A confirmed match is a record that satisfies the complete query under the published generation.

Posting trees may produce confirmed predicate matches without scanner candidates. Once all query conditions are proven, do not revalidate them from the file just because the result came from an index.

Membership, source loading, and logical decoding are separate costs. Knowing an ID matches does not mean its full logical data is resident.

Every returned match identifies its collection/record, source file, byte range, recorded modification time, and relevant source metadata. Provide matched logical field paths when requested; distinguish positive value evidence from a negative/presence predicate's satisfied condition. Do not manufacture positive text locations for a purely negative match.

Support three selectable record return modes:

| Mode | Returned payload | Source work after membership is known |
|---|---|---|
| Record bytes | Original bytes of the complete record range, unchanged | Reuse bytes already read, or read the range; no logical parse solely for byte output |
| Logical record | Typed nested logical structure, optionally serialized to JSON-compatible data | Reuse the verification parse, or parse once if the match was resolved entirely from indexes |
| Source locator | File/path reference, start/end offsets, revision and metadata; no record body | No source read when membership and requested evidence are already available |

Whole record bytes means the record's range, not the entire containing file. Source-locator mode also serves callers that only need identities and locations.

Collect complete requested matched-field evidence for returned records. A global raw filter does not by itself identify the matching fields. Derive evidence from field-aware postings or the scanner; this may require a page-record parse even when membership alone was index-proven. Count-only queries do not collect per-record evidence.

If verification already parsed a returned record, carry its evidence and chosen payload forward. Group index-proven records needing logical payload by file as well. Keep source work for discarded results out of the output stage whenever possible.

If source metadata changes during access, do not return content from new bytes with old match/range evidence. Return the explicit stale/incomplete outcome described below; reopening can establish a new consistent snapshot.

## 14. Caching and repeated execution

Keep independent byte budgets for prepared query plans, verified result membership/order, and optional source/decoded-content caches. The repository owns these caches; entries are scoped by selected collection states.

The principal result cache stores verified record IDs/slots, optionally matched-field evidence, and complete ordered sequences. It does not need to store all record contents.

Keys include resolved query semantics, selected collections and generations, binding versions, and ordering/collation when relevant. Looking up a cached result must not stat, open, or hash every file. Return mode is separate from membership; reuse membership across bytes/logical/locator requests.

Cache complete ordered sequences independently of offset and limit, then slice them for repeated pagination. Share immutable buffers rather than copying the full list per hit. Cache empty verified results.

Never store approximate candidates or an incomplete scan as a complete verified result. A partial page/prefix must be marked partial and must not supply arbitrary later pages or an exact COUNT.

Large result sequences need bounded admission, eviction, and memory accounting. Do not force every first-page query to enumerate and retain all matches merely to populate a cache. Start with collection-generation invalidation; add finer dependency tracking only after measurements justify its complexity.

Reuse prepared literal matchers, field selectors, case transformations, custom parsers/scanners, and sort definitions. Create worker state once per useful batch, not once per candidate.

## 15. Freshness, consistency, and future index mutability

### Required now: correct inventories, construction, and restoration

Normal queries search the repository snapshot established at open. They do not promise immediate visibility of external filesystem edits. Implement startup construction/restoration and the file-to-record index; do not implement a runtime update method, whole-file update ingestion, incremental record mutation, live watchers, or periodic background reconciliation. Closing and reopening may rebuild an affected collection or the entire repository when source revisions changed.

Use modification time as the initial revision mechanism, with size/range validation. Reconcile files during startup/restoration and before publishing a rebuilt generation. Check metadata around source access. Do not add mandatory hashing, per-query collection-wide metadata checks, or a historical source-copy store.

File creation, modification, replacement, deletion, and rename must be reflected after the next successful open/reopen. During startup reconciliation, rebuild any stale collection state as needed, including its record mappings, postings, bigrams, sorting entries, and planner statistics. This is snapshot validation and reconstruction, not a runtime updater. Do not preserve offsets across a source revision without a proven framing rule.

Construct new state privately and publish it consistently. A query must not combine new record offsets with old posting sets or new raw text filters with stale ICU order. Generations bind record slots, source revisions, statistics, indexes, and cache entries together. If several collections include a changed file, publish their corresponding states consistently.

If a source changes during verification/output, return an explicit stale/incomplete outcome. Do not silently update indexes inside a query. Reopening can establish a new snapshot. Bound any source-access retries. Unreadable, malformed, or unstable records are visible diagnostics, not silent nonmatches. Incomplete results cannot enter the complete-result cache.

Old cached locators remain tied to their recorded revision. An immutable index generation does not make the live file historical. Do not promise old logical contents unless they are actually retained.

### Future update boundary: complete files

Later updates will feed a complete file, identified as a new file or a replacement revision of an existing file. The source reader determines the records and their new byte ranges. Do not design the future contract around caller-supplied record diffs, individual field edits, or byte patches. No update entry point or update execution is to be implemented now.

Document the future replacement sequence:

1. Use the file-to-record index to identify all old record slots for the file in every affected collection. This must work without rereading the previous file contents.
2. Frame and extract the complete new file once, producing a new record set and all configured index contributions. A file may change from one record to many, many to none, or vice versa; never assume the previous ordinals, IDs, or offsets remain valid.
3. Prepare a consistent new state that removes or suppresses all old record contributions and installs the new record mappings, postings, bigrams, sorting entries, coverage, and statistics. Publish the replacement only after successful preparation; extraction failure must not be treated as a valid empty replacement.
4. Make the new generation visible consistently and invalidate dependent caches. Existing readers retain their captured index state, with the existing source-revision safeguards for reads from live files.

A future file deletion is a separate explicit removal of the file identity and all mapped records; it is not the same as feeding a valid file containing zero records. Whole-file ingestion does not require an API for editing or writing logical records back to their source format.

### Design now for later index changes

Keep a practical route to adding/removing the records derived from a complete file without rebuilding every index in the repository. Do not implement a transaction system, update log, mutable overlay, or concurrent writer in this assignment.

Document how the chosen structures can support the following:

| Area | Future mutation consideration |
|---|---|
| Record/file inventories | Use the required file-to-record index to replace the complete record set contributed by one file; distinguish logical identity from generation-specific slots and byte ranges |
| B-tree postings | Insert/remove record membership for typed keys, presence, null, and derived values; remove empty entries where appropriate |
| Bigram filters | Add and remove record contributions without leaving stale candidates; consider replaceable blocks/partitions or a bounded delta over the existing compact layout |
| Sorting lists | Recompute changed keys and insert/remove ordered entries; an immutable sorted base with later merging or replaceable partitions is acceptable |
| Planner statistics and coverage | Update counts, summaries, and coverage consistently with the published index state |
| Caches and prepared operations | Invalidate by relevant generation/binding changes; prevent a reused slot from referring to a different record through an old cached result |
| Persistence | Version the layout so later partitioning or mutation metadata can be added without silently misreading an older snapshot |

Prefer a small separation between builders, published query views, and generation replacement. Use existing file-to-record mappings and explicit index ownership. A future implementation can choose direct updates, replacement partitions, or an immutable base with bounded changes after measurement. Do not prescribe one mutation algorithm for every index family.

Compact, contiguous, immutable buffers remain appropriate for fast queries. Avoid decisions that make the full repository's layout inseparable from one permanently fixed record count, or make replacing one collection require rewriting unrelated collections. At the same time, do not store large reverse-contribution maps, add per-record locks, or add dynamic dispatch to inner query loops solely for a future updater. Explain any unavoidable tradeoff and its expected memory or query cost.

Future deletions and replacements must suppress old contributions across every index family before the new state becomes visible. Slot reuse must be generation-safe. If an overlay is introduced later, it needs bounded growth, correct suppression of replaced/deleted records, and compaction before it degrades repeated-query latency.

Include a short mutation design note with the implementation: describe how a supplied complete file will replace its mapped record set, the likely index rebuilding unit, how readers retain consistent state, and any current structure that would need replacement. Full rebuilding at open today must be an explicit implementation choice, not an undocumented assumption throughout query code.

### Deferred capability and acceptance

Update ingestion/execution, incremental mutation, live filesystem watchers, concurrent writers, and compaction are deferred. Do not implement them voluntarily as part of this assignment. The file-to-record index, its persistence, and its consistency checks are current requirements.

Current acceptance requires correct open/reopen behavior, rejection of invalid source ranges, consistent generation publication, and isolation of caches from incompatible generations. Query, index-build, and index-restore benchmarks remain mandatory. Do not sacrifice measured repeated-query performance to speculative mutation machinery.



## 16. Persistence and restoration

Serialized snapshots accelerate startup; queries run on in-memory indexes and local files remain authoritative.

Persist the logical model/configuration version, internal registration/binding identities, file metadata and modification times, record IDs/ranges and slot mappings, the file-to-record index, posting trees, special presence/null entries, bigram variant/alphabet, ICU keys/lists and collation version, and coverage. Validate that the restored file-to-record and record-to-file mappings agree.

Use a versioned envelope and suitable direct encodings for bulk structures. Reuse the supplied bigram serialization and established bitmap serialization. Do not serialize transient Rust addresses or rely on an unstable in-memory object layout.

Write atomically through a temporary file. Validate section sizes, offsets, checksums, and domain compatibility before use. Corruption or incompatibility produces a controlled rebuild or clear error.

Restore into private state and reconcile current files before publication. Added, modified, and deleted files must update their records and all index contributions. A successful deserialization is not proof that the collection is current.

Reconciliation may rebuild each affected collection completely in this version; efficient incremental restoration after edits is deferred with index mutation. Report any such rebuilding inside total startup-to-query-ready time. Restoring an unchanged compatible snapshot must still use its stored indexes rather than always rebuilding them.

Measure both snapshot decoding and total startup-to-query-ready time, including discovery, modification-time reconciliation, initialization, and index reconstruction. Preserve enough recorded state to restore efficiently; do not secretly rebuild the corpus outside the measured interval.

## 17. Dependencies and memory layout

Use stable Rust, a recorded compiler version, and a committed Cargo lockfile. Favor established low-level components.

| Responsibility | Recommended technology |
|---|---|
| Ordered typed posting dictionary | `std::collections::BTreeMap` |
| Record sets | `roaring`, compact sorted vectors, or supplied compatible bitmap storage |
| Bigram columns and operations | Supplied implementation |
| ICU string order and precomputed keys | ICU4X `icu_collator`, with locale/data support |
| Unicode case-insensitive binding | ICU4X `icu_casemap`; define/version the selected folding behavior |
| Dates, datetimes, Unix seconds | `time` |
| Literal substring scanning | `memchr` reusable substring finders |
| Parallel construction and substantial scans | `rayon` with bounded concurrency |
| File discovery | `ignore` or an equivalent established walker, configured explicitly |
| Change hints | `notify`, with reconciliation |
| Published immutable state | `Arc` and a short publication lock; `arc-swap` if beneficial |
| Configuration/snapshot metadata | `serde` and a compact versioned encoding such as `postcard` |
| Generated correctness cases | `proptest` |
| Performance measurement | Dedicated release-mode query/build/restore runner |

ICU is the chosen sorting technology. Verify the pinned crate's supported interfaces and data provisioning rather than relying on an unspecified latest release. Neither SQLite nor PocketBase is a runtime dependency; PocketBase supplies syntax inspiration.

A clear lexer plus recursive-descent/Pratt parser is sufficient. Do not introduce an asynchronous runtime or public extension framework solely for this local core.

Use compact IDs, contiguous inventory/column arrays, shared file paths, and interned field definitions. Borrow parsed values when possible. Reuse allocation-heavy scratch buffers. Avoid copying complete bitmaps, logical records, result lists, or strings between pipeline stages.

Bound file-read buffers, worker queues, builder intermediates, caches, winner payloads, and retained generations. SIMD paths require portable fallbacks. Add unsafe code only with documented invariants and necessary tests.

## 18. Correctness and performance validation

### Correctness

Maintain an independent scanner/reference evaluator over small fixtures. Compare optimized identifiers, COUNT, ordering, pagination, payload modes, and matched fields with that reference.

Required cases include:

- Multiple records per file and independent parsing from each stored range.
- File-to-record and record-to-file mapping agreement, empty-file entries, multiple collections sharing a file, restored mappings, and efficient per-file enumeration without scanning the complete record inventory.
- Non-ASCII bytes before a record, framing separators, empty files, and ranges at file boundaries.
- Duplicate intrinsic IDs, revision-bound generated IDs, and changes that move every subsequent range.
- Date versus datetime versus unixtime, timezone offsets, numeric precision, homogeneous/nullable arrays, nested objects, missing/null/empty distinctions.
- B-tree equality/ranges, presence/null special keys, array duplicates/cardinality, universal versus existential negation.
- Exact substring FTS, longer posting values containing the query, raw punctuation/whitespace, short strings, Unicode, and segment boundaries.
- Custom value parsing/index/scanner equivalence, including a transformation whose match is absent from original encoded bytes.
- ICU locales, equal collation keys, stable ties, descending order, multi-field sorting, and collation-version invalidation.
- Index coverage gaps, AND/OR/negation, cache invalidation, empty cached results, and partial-result rejection.
- All three return modes with no repeated candidate decode.
- Metadata/data-name collisions, quoted names beginning with `@`, inventory-only metadata predicates, and metadata sorting.
- ASCII/Unicode bigram result equivalence, non-ASCII fallback, and mismatched-alphabet snapshot rejection.
- Snapshot restore and file edits/additions/deletions/replacements reflected after reopening; rebuilding affected collections is allowed. Reopened whole files with different record counts and boundaries must produce complete new file-to-record mappings.
- Modification during build/verification, malformed records, truncated snapshots, and bounded retries.

Instrument reads and parses in targeted tests: one record parse during fused index construction; at most one candidate parse per query; zero verification parses for index-proven predicates; no second parse when returning an already verified logical record.

Review the future whole-file mutation boundary described in Section 15. Do not implement or require an updater, watcher, or compaction mechanism for current acceptance. Test the required file-to-record index and generation/cache isolation through construction, restoration, and reopening after source changes.

### Optimizer acceptance

Use targeted fixtures with known posting sizes and instrumented source access to verify these requirements. Prefer deterministic work-count assertions over fragile elapsed-time limits or snapshots of every internal plan detail.

- Equivalent permutations of conjunctions/disjunctions return the same records, COUNT, order, payloads, and requested field evidence. Written order must not force expensive scans before an available economical restriction.
- A selective exact lookup preceding a residual substring or unindexed predicate limits verification to its survivors, including when the expensive predicate appears first in the input. A fully covered empty exact conjunct requires no source access.
- A broad range/dictionary path can be bypassed after another term produces a tiny candidate set. A rare compatible bigram restriction can precede a broad posting. Controlled tests exercise both choices.
- Partial coverage, unindexed OR branches, approximate negation, array quantifiers, and differently transformed custom bindings retain scanner-equivalent results. Approximate candidates never become false exact answers through reordering.
- Scalar range simplification is tested separately from existential array predicates whose conditions can use different elements. Invalid query literals/bindings are rejected consistently regardless of term order.
- Numeric precision and distinct temporal types survive physical specialization; numeric-looking strings and ICU-equivalent strings do not acquire different equality semantics.
- Actual cardinality can change a remaining execution choice without rerunning completed work or parsing a record twice. Several candidate records in one file share source access.
- COUNT and sorting prevent invalid limit-based early exits; prepared-plan reuse and verified-result reuse remain distinguishable.

Keep these targeted tests separate from the fixed 150-case workload. Do not change its IDs, literals, or options to fit the optimizer. Use the independent evaluator with indexes disabled as the correctness reference; do not implement the reference by calling the optimized plan through a different entry point.

### Benchmarks

Use the fixed 150-case Musenalm workload in Section 21 for final acceptance and timing reports. Smaller explicitly identified subsets are allowed during development, but do not replace the final run. Keep small generated single-record/multi-record fixtures for correctness and additional source-access measurements; do not alter the Musenalm dataset to manufacture those cases.

Measure:

| Category | Required result |
|---|---|
| Resident indexes, verified cache disabled | Query latency, exact posting results, candidate count, verification and parse cost |
| Query optimization | Preparation/planning cost, chosen access paths and term order, estimated versus actual candidate reduction, and avoided source work |
| Warm verified cache, including 64 MiB | Repeated query and ordered-pagination latency |
| Return modes | Locator, raw record bytes, and logical records measured separately |
| Multi-record source access | Sparse/dense candidate ranges, coalescing, file opens, bytes read, per-record parses |
| Index construction | Total time, phases, throughput, ICU-key cost, steady-state and peak memory |
| Restoration | Fresh-process decoding and full startup-to-query-ready time |
| Reopening after source changes | Correct reconstructed inventories and index state; report the rebuilding unit and its startup cost |

Runtime whole-file ingestion and incremental update benchmarks are deferred with their implementation. Include file-to-record index construction, persistence, restoration, and memory in the normal index measurements; do not omit this required inventory from reported costs.

COUNT is record count. Report physical files opened as a separate work counter. Distinguish exact posting matches, unverified candidates, records scanned, total matches, returned records, source bytes read, and payload decodes.

Evaluate planner improvements primarily with verified-result caching disabled; include prepared-plan cache status and end-to-end latency. Warmed 64 MiB result-cache timings remain required, but cannot substitute for evidence that index selection and term ordering improve actual execution. Save representative plans and actual work counters. Use small, separately labeled term-order and selectivity experiments where needed; no additional historical 150-query suite is required.

Use a persistent process for repeated-query measurements. Separate filesystem-cache state, resident index state, verified-result cache state, and content cache state. Do not rebuild indexes for every timed query.

Reuse an applicable recorded current-state baseline. If none exists, record one before changing established behavior. For a new project, the first correct implementation establishes the initial query/build baseline, and the first correct restore implementation establishes its restore baseline. Preserve binaries/commits for later comparison.

Use the same corpus, query semantics, return mode, compiler flags, worker count, and hardware. Record corpus/configuration fingerprints and raw measurements. Alternate comparable runs rather than timing them concurrently. Use multiple build/restore process runs, normally at least five when practical.

Report absolute microseconds/milliseconds first, percentages second, plus variability. A 7% change on a short query can be a small absolute difference. Do not invent a universal percentage threshold or infer a regression from one noisy sample.

The final benchmark is mandatory: execute all 150 specified Musenalm cases, plus index construction and restoration measurements, for both ASCII and Unicode bigram settings. This replaces the earlier small-workload restriction. Save the full results and print every query's timing summary at the end as required in Section 20. Keep correctness testing distinct from timing. No extra historical query suite is required; state unavailable measurements explicitly.

## 19. Implementation sequence and completion

Implement on a separate branch, for example `development/local-file-search-core`. Preserve unrelated work. Every coherent stage must pass its relevant checks, be committed separately, and be pushed separately to the configured remote. Keep a short progress/measurement record. Do not merge automatically.

The numbered list below summarizes the required capabilities. Use the detailed phases and commit plan in Section 22 at the bottom of this file as the authoritative execution order and commit schedule.

1. **New workspace and fixture integration:** Inspect any useful previous implementation under `old/`, especially `old/crates/core/`. Establish the new workspace with `core/`, preserve root-level `examples/`, read the existing Musenalm schemas, add the test CLI registration layer, and commit the explicit Q001–Q150 workload manifest. Choose and document reuse freely within the new specification.
2. **Logical model and ownership:** Repository/query space, collection definitions, separate file/record inventories, the required file-to-record index and reverse source mapping, prefixed metadata, temporal types, identity and standalone ranges, reference readers/evaluator.
3. **Languages and bindings:** Separate FTS/FILTER inputs, PocketBase-style operators and quantification, typed literal parsing, defaults, internal custom registrations, return-mode contracts.
4. **Fused construction and postings:** One parse per record, B-tree values/presence/null/cardinality, coverage, exact planning, initial query/build baseline.
5. **Supplied bigram integration:** Record-level raw extraction, repository ASCII/Unicode selection, normal/skip filters, conservative candidate selection, prepared scanners, grouped range reads.
6. **Query optimizer:** Typed predicates, access-path enumeration, inexpensive statistics, cost/selectivity estimates, AND/OR reordering, restricted residual evaluation, bounded adaptation, and EXPLAIN diagnostics. Prove candidate reduction and source-access savings with targeted tests and measurements before relying on result caches. Do not postpone this stage to final tuning.
7. **ICU sorting and results:** Sort keys/lists, stable global order, record COUNT, pagination, matched-field evidence, and all three payload modes without repeated decode. Integrate filtering-versus-ordering choices into the planner.
8. **Reuse, freshness, and future whole-file boundaries:** Prepared plans, verified/ordered caches, shared buffers, consistent publication, generation-bound planner statistics, and modification-time freshness. Document how future complete-file ingestion will replace the records located through the file-to-record index. Do not implement runtime updates, refresh ingestion, or watchers.
9. **Persistence:** Versioned snapshots, range/domain and statistics validation, reconciliation, full restoration measurement and retained restore baseline.
10. **Measured refinement:** Correctness/property tests, allocation/copy/parse improvements where needed, all 150 final Musenalm cases in both alphabet/cache profiles, saved and fully printed timings, query/build/restore comparisons, planner overhead and execution analysis, documentation.

The final report includes the branch and pushed commits, reuse from `old/`, architecture and scanner/index registrations, grammar and type semantics, optimizer choices and representative execution plans, freshness/offset limitations, the future mutation design and deferred capabilities, tests, all 150 query summaries and raw samples for each profile, build/restore results, memory costs, and remaining limitations.

Completion means a working record-oriented search core: resident B-tree postings, ICU sorting lists, and raw/custom bigram filters serving repeated queries over independently addressable local record ranges.

A planner that merely follows written term order, always chooses one fixed index family, or leaves all useful candidate reduction to the scanner does not satisfy completion. Demonstrate typed access-path selection and economical term ordering through both controlled work-count tests and the saved Musenalm plans.

## 20. Musenalm fixture, test CLI, and complete timing reports

### Fixture and schema authority

Use the existing test repository at `examples/musenalm/` in its current organization. Do not migrate, flatten, regenerate, rename, or rewrite its data to fit the new core.

The inspected layout has collection schemas under `examples/musenalm/.objekte/collections/`, collection data under `examples/musenalm/data/`, and collections named `agents`, `contents`, `entries`, `items`, `pages`, `places`, and `series`. Treat the schemas actually present in the checkout as authoritative if they have changed.

Read each schema's `x-objekte` declaration for collection name, source path, format, and identity policy. Read property types, required fields, nullability, array/object structure, `index`, and `x-index-paths` to prepare registrations. Resolve local schema references when present. Report unsupported declarations rather than silently discarding them.

The currently inspected declarations select JSON and filename identity. Therefore register a JSON source reader, standalone-record scanner, logical value extractor, and text extractor in the CLI integration layer. Register every reader/scanner required by the actual declared source formats; do not assume filename suffix alone supplies the registration. No unused XML/HTML/Markdown implementation is required.

Inline markup within a JSON string remains a string. Likewise, schema fields declared as strings remain strings even when they contain timestamp-looking values. Do not silently convert the entire fixture to temporal types; test mandatory temporal types separately, and use the explicitly typed `@file.created`/`@file.modified` metadata in this workload.

Honor the fixture's filename identity: `@record.id` is the filename stem for its single-record JSON files. This is an explicitly configured identity provider, so the generic revision-bound fallback ID does not apply. Embedded arrays of objects are part of their parent record; do not promote them into independent records. Do not merge standalone `items` records with embedded `entries.items`.

The CLI must preserve fractional numeric values, notably `contents.numbering`. It must traverse nested logical fields and array paths declared in `x-index-paths`; schema path syntax such as `[*]` must be translated into internal selectors rather than treated as a literal field name.

Map explicit lookup-index declarations to B-tree postings. Keep unindexed fields searchable through conservative raw candidates or scanning. Register ICU sorting lists for the fields explicitly sorted in the workload, using one documented locale (default `de` for this fixture) and recorded options. Register presence/null/derived indexes according to an explicit, reported policy; do not secretly add a posting tree for every benchmark predicate to make results appear faster.

Register one named test binding, `agents.name:folded`, to demonstrate the complete custom path. It parses/folds the query value, builds its own folded-text bigram path, and verifies with the identical folding scanner over the existing name values. Q070 exercises this registration. This is declared CLI test configuration, not a change to fixture files. Include its build/memory cost and its definition in the report.

Keep this schema adapter, JSON reader selection, fixture-specific registration, and workload orchestration in the test CLI/support layer. The generic core must not depend on Musenalm names or on this schema annotation vocabulary.

### Test CLI

Provide an executable example or a small dedicated test binary using the new core; a suitable location is `core/examples/musenalm_cli.rs` in the new workspace. Reused implementation components may be integrated according to Section 1.

The CLI must:

1. Accept the repository root, defaulting to `examples/musenalm/`.
2. Load schemas and print/save a registration summary: collection paths, file/record counts, source formats/scanners, type hints, indexed paths, sorting definitions, custom bindings, and effective bigram alphabet.
3. Build or restore a resident repository once per profile and execute queries repeatedly in that process.
4. Support an individual FTS/FILTER request and execution of the fixed workload manifest.
5. Expose alphabet, verified-cache size, repetitions, return mode, and result-directory controls.
6. Preserve record bytes, logical data, and locator modes exactly as specified, including when implementation components are reused from `old/`.
7. Print the command/configuration needed to reproduce a run.
8. Provide EXPLAIN and EXPLAIN ANALYZE diagnostics showing typed bindings, chosen access paths, term ordering, candidate reductions, residual work, and source access.

Commit an explicit machine-readable manifest, for example `core/benches/musenalm_queries.json`, containing the 150 cases in Section 21. Each entry contains ID, collection scope, separate FTS and FILTER values, selected FTS fields, sort, limit, offset, return mode, COUNT, and evidence options. Do not generate an undocumented set of queries at runtime.

Validate paths/types and configuration before measurement. Record empty results honestly; the list does not assert a match count. If a supplied ID or literal is absent in a later fixture revision, it remains a valid query. If a schema field is renamed or removed, report the mismatch and document any necessary workload revision rather than silently substituting a different predicate.

A zero-result query is not a skipped query. A failed or unsupported query gets a failure status and diagnostic, not a zero-millisecond successful timing. Preserve all 150 IDs in the final report, continue reporting independent cases where possible, and exit unsuccessfully if required cases fail.

### Measurement profiles and reporting

Execute all 150 unique cases under these four profiles:

| Bigram alphabet | Verified-result cache |
|---|---|
| ASCII | Disabled |
| ASCII | 64 MiB, warmed |
| Unicode | Disabled |
| Unicode | 64 MiB, warmed |

Build/restore the appropriate index state once for each alphabet and keep it resident while querying. Cache-profile changes do not justify rebuilding the corpus per query. Use the same literals, options, selected fields, logical results, and collation configuration across profiles. Disable optional source-byte and decoded-record caches in these four standard profiles so their effects are not confused with verified-result caching; separately labeled content-cache experiments are allowed.

Use five measured repetitions per case/profile by default, configurable and recorded. Prepare/warm the resident filesystem/index state outside query measurements. With result caching disabled, keep it disabled throughout. For the warmed profile, run an untimed preparation query for that case, then measure repeated requests. If an entry is not admitted or has been evicted, report the actual miss; do not label it a hit or increase the configured budget silently.

Record total query time from request submission through requested COUNT/page/payload completion, excluding terminal printing and report-file writes. Additional planning, execution, verification, ordering, and materialization phase timings are useful. Report actual cache outcomes and work counters. Query preparation and matched-field work must not disappear from the measured boundary without an explicit separately labeled metric.

Record prepared-plan cache hits independently from verified-result cache hits. Obtain representative execution diagnostics in separate runs outside primary latency samples, using the same captured configuration and source state. A cache hit may explain why a saved diagnostic executes no term operations; retain an uncached representative plan for that case as well. Mark diagnostic runs explicitly and do not include them in the five primary timing samples.

The workload requests COUNT, so its time includes complete record counting. Do not interpret these measurements as early-exit page-only latency. Additional page-only experiments may be reported separately without replacing the fixed cases.

Save, at minimum, to a unique directory outside the input fixture, such as `bench/results/musenalm/<run-id>/`:

- `run-manifest.json`: commit/build environment, actual commands, workload/schema/configuration fingerprints, collection counts, scanner/index registrations, ICU configuration, alphabet/cache profiles, and repeat counts.
- `queries.json`: exact resolved 150-case input manifest used for this run.
- `measurements.jsonl`: one raw timing/status/counter sample per case, profile, and repetition.
- `query-results.csv`: all 150 cases for every profile with minimum/median/maximum time, samples, total matches, returned records, candidates, source reads/bytes, parse counts, and cache outcomes.
- `query-plans.jsonl`: representative resolved plans and execution diagnostics keyed by case/profile, including per-term types/bindings, chosen paths, order, estimates, actual cardinalities, coverage, residual verification, and adaptive changes.
- `index-results.json`: construction/restoration time, phases, memory, and serialized index size for both alphabets.
- `REPORT.md`: complete human-readable query timing tables, build/restore results, optimizer findings with absolute costs and candidate/source-work evidence, configuration, failures, and reproducible commands.

**At the end of the CLI run, print all 150 query summaries to the terminal**, including each case's query text/options and absolute timings for every profile. Print the construction/restoration summary and saved report paths as well. Do not show only aggregate averages, a fastest/slowest subset, or a truncated table. Persist raw samples progressively so a later failure does not discard earlier measurements.

Use one row per case with profile columns or separate complete profile tables. Values must be inspectable without rerunning the CLI. Do not withhold reports because performance is unfavorable; the user will evaluate the timings.

Keep the existing before/after baseline rule: reuse a compatible current-state recording, otherwise record one once a runnable new-core version exists. Construction and restoration require before/after evidence as those paths evolve. Never invent results for this specification or claim the workload has run before the CLI exists.

The original fixture remains unchanged. For multi-record correctness and source-access benchmarks, create a separate generated fixture outside `examples/musenalm/`, using selected records or independent small inputs. These supplementary cases do not replace any of the 150 Musenalm queries.

## 21. Fixed 150-query Musenalm workload

The following list is the required manifest, Q001 through Q150. All fields were checked against the inspected collection schemas; runtime validation remains necessary for the actual checkout. The entry/place/agent reference literals come from existing fixture records.

Defaults for every case unless its Options column overrides them:

- Collection `*` means all declared Musenalm data collections; other names select one collection.
- FTS uses `raw`; a displayed quoted FTS value is the literal string after removing its presentation quotes. The em dash means no FTS/FILTER component.
- FTS and FILTER, when both present, are combined with AND.
- LIMIT 50, OFFSET 0, COUNT enabled, return mode locator, and matched-field evidence requested.
- No SORT BY means the engine's deterministic collection/record order.
- SORT entries use the configured ICU locale for strings and typed ordering for other values.
- Q137–Q139 deliberately hold the predicate/order constant while comparing bytes, logical, and locator output.
- Cases are benchmark requests, not asserted performance or result-count claims.

### Q001–Q030: FTS substring searches

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q001 | `*` | `"Almanach"` | — | — |
| Q002 | `*` | `"Taschenbuch"` | — | — |
| Q003 | `*` | `"Leipzig"` | — | — |
| Q004 | `*` | `"Wien"` | — | — |
| Q005 | `*` | `"Goethe"` | — | — |
| Q006 | `*` | `"ü"` | — | — |
| Q007 | `*` | `"a"` | — | — |
| Q008 | `*` | `"__objekte_missing_7c9f31__"` | — | — |
| Q009 | `places` | `"Leipzig"` | — | — |
| Q010 | `places` | `"Wien"` | — | — |
| Q011 | `agents` | `"Goethe"` | — | — |
| Q012 | `agents` | `"Schiller"` | — | — |
| Q013 | `agents` | `"Müller"` | — | — |
| Q014 | `agents` | `"Text"` | — | — |
| Q015 | `agents` | `"https://"` | — | — |
| Q016 | `series` | `"Almanach"` | — | — |
| Q017 | `series` | `"Taschenbuch"` | — | — |
| Q018 | `entries` | `"Vergnügen"` | — | — |
| Q019 | `entries` | `"Almanach"` | — | — |
| Q020 | `entries` | `"1808"` | — | — |
| Q021 | `entries` | `"WIEN"` | — | — |
| Q022 | `entries` | `"unbezeichnet"` | — | — |
| Q023 | `contents` | `"Liebe"` | — | — |
| Q024 | `contents` | `"Frühling"` | — | — |
| Q025 | `contents` | `"Lied"` | — | — |
| Q026 | `contents` | `"Der"` | — | — |
| Q027 | `contents` | `"1808"` | — | — |
| Q028 | `items` | `"Theodor Springmann Stiftung"` | — | — |
| Q029 | `pages` | `"Musenalm"` | — | — |
| Q030 | `entries` | `"Taschenbuch"` | — | `FTS fields: preferred_title` |

### Q031–Q060: Scalar, metadata, and typed posting predicates

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q031 | `places` | — | `fictional = false` | — |
| Q032 | `places` | — | `fictional = true` | — |
| Q033 | `places` | — | `edit_state = "Edited"` | — |
| Q034 | `places` | — | `@record.id = "d7kbujq7399g22j"` | — |
| Q035 | `places` | — | `musenalm_id > 100` | — |
| Q036 | `places` | — | `musenalm_id <= 100` | — |
| Q037 | `agents` | — | `corporate_body = true` | — |
| Q038 | `agents` | — | `corporate_body = false` | — |
| Q039 | `agents` | — | `fictional = true` | — |
| Q040 | `agents` | — | `fictional = false` | — |
| Q041 | `agents` | — | `edit_state = "Review"` | — |
| Q042 | `agents` | — | `profession = "Text"` | — |
| Q043 | `agents` | — | `musenalm_id >= 1000` | — |
| Q044 | `entries` | — | `year = 1808` | — |
| Q045 | `entries` | — | `year < 1800` | — |
| Q046 | `entries` | — | `year >= 1800 && year < 1850` | — |
| Q047 | `entries` | — | `@record.id = "00fx8ln651na0e3"` | — |
| Q048 | `entries` | — | `edit_state = "Edited"` | — |
| Q049 | `entries` | — | `pseudonym = false` | — |
| Q050 | `contents` | — | `entry = "00fx8ln651na0e3"` | — |
| Q051 | `contents` | — | `year = 1808` | — |
| Q052 | `contents` | — | `year >= 1800 && year < 1850` | — |
| Q053 | `contents` | — | `numbering = 1` | — |
| Q054 | `contents` | — | `numbering > 1 && numbering < 2` | — |
| Q055 | `contents` | — | `numbering >= 10` | — |
| Q056 | `contents` | — | `pseudonym = true` | — |
| Q057 | `items` | — | `basis = true` | — |
| Q058 | `items` | — | `public = true` | — |
| Q059 | `items` | — | `owner = "Theodor Springmann Stiftung"` | — |
| Q060 | `series` | — | `frequency = "jährlich"` | — |

### Q061–Q080: String, wildcard, and custom-binding predicates

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q061 | `places` | — | `name = "Leipzig"` | — |
| Q062 | `places` | — | `name ~ "zig"` | — |
| Q063 | `places` | — | `name:lower ~ "LEIPZIG"` | — |
| Q064 | `places` | — | `name ~ "Leip%"` | — |
| Q065 | `places` | — | `name !~ "zig"` | — |
| Q066 | `agents` | — | `name ~ "Goethe"` | — |
| Q067 | `agents` | — | `name:lower ~ "goethe"` | — |
| Q068 | `agents` | — | `name ~ "%Goethe%"` | — |
| Q069 | `agents` | — | `pseudonyms ~ ""` | — |
| Q070 | `agents` | — | `name:folded ~ "GOETHE"` | — |
| Q071 | `entries` | — | `preferred_title ~ "Almanach"` | — |
| Q072 | `entries` | — | `preferred_title:lower ~ "TASCHENBUCH"` | — |
| Q073 | `entries` | — | `title_statement ~ "WIEN"` | — |
| Q074 | `entries` | — | `annotation !~ "Beschädigung"` | — |
| Q075 | `entries` | — | `place_statement ~ "Wien"` | — |
| Q076 | `contents` | — | `preferred_title ~ "Liebe"` | — |
| Q077 | `contents` | — | `preferred_title:lower ~ "FRÜHLING"` | — |
| Q078 | `contents` | — | `incipit_statement ~ "Der"` | — |
| Q079 | `series` | — | `title ~ "Taschenbuch"` | — |
| Q080 | `pages` | — | `data.description:exists = true` | — |

### Q081–Q105: Array membership and nested fields

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q081 | `entries` | — | `places ?= "d7kbujq7399g22j"` | — |
| Q082 | `entries` | — | `places ?!= "d7kbujq7399g22j"` | — |
| Q083 | `entries` | — | `!(places ?= "d7kbujq7399g22j")` | — |
| Q084 | `entries` | — | `language ?= "ger"` | — |
| Q085 | `entries` | — | `language:each = "ger"` | — |
| Q086 | `entries` | — | `carrier_type ?= "Band"` | — |
| Q087 | `entries` | — | `content_type ?= "Text"` | — |
| Q088 | `entries` | — | `agents.agent ?= "lngx6eyfw5qrzas"` | — |
| Q089 | `entries` | — | `agents.type ?= "Verlag"` | — |
| Q090 | `entries` | — | `agents.uncertain ?= true` | — |
| Q091 | `entries` | — | `items.owner ?= "Theodor Springmann Stiftung"` | — |
| Q092 | `entries` | — | `items.public ?= true` | — |
| Q093 | `entries` | — | `items.media ?= "Original"` | — |
| Q094 | `entries` | — | `musenalm_deprecated.status ?= "Original vorhanden"` | — |
| Q095 | `contents` | — | `contributors.agents ?= "lngx6eyfw5qrzas"` | — |
| Q096 | `contents` | — | `contributors.type ?= "Autor:in"` | — |
| Q097 | `contents` | — | `contributors.conjecture ?= true` | — |
| Q098 | `contents` | — | `contributors.uncertain ?= true` | — |
| Q099 | `contents` | — | `musenalm_type ?~ "Lied"` | — |
| Q100 | `contents` | — | `language ?= "ger"` | — |
| Q101 | `series` | — | `entries ?= "00fx8ln651na0e3"` | — |
| Q102 | `series` | — | `related_entries.entry ?= "00fx8ln651na0e3"` | — |
| Q103 | `items` | — | `media ?= "Original"` | — |
| Q104 | `agents` | — | `data.gnd.gender.label ?= "Weiblich"` | — |
| Q105 | `agents` | — | `data.gnd.type ?~ "Person"` | — |

### Q106–Q125: Presence, null, cardinality, and metadata

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q106 | `agents` | — | `data:null = true` | — |
| Q107 | `agents` | — | `data:exists = true && data:null = false` | — |
| Q108 | `agents` | — | `data.gnd.preferredName:exists = true` | — |
| Q109 | `agents` | — | `@file.size >= 4096` | — |
| Q110 | `entries` | — | `edit_fielddata:null = true` | — |
| Q111 | `entries` | — | `@record.byte_start = 0` | — |
| Q112 | `entries` | — | `musenalm_deprecated:null = true` | — |
| Q113 | `entries` | — | `places:length = 0` | — |
| Q114 | `entries` | — | `places:length > 0` | — |
| Q115 | `entries` | — | `items:length >= 2` | — |
| Q116 | `contents` | — | `contributors:length > 0` | — |
| Q117 | `contents` | — | `contributors:length = 0` | — |
| Q118 | `contents` | — | `contributors:length >= 3` | — |
| Q119 | `contents` | — | `@file.modified >= "2026-01-01T00:00:00Z"` | — |
| Q120 | `contents` | — | `incipit_statement = ""` | — |
| Q121 | `series` | — | `entries:length >= 3` | — |
| Q122 | `series` | — | `related_entries:length > 0` | — |
| Q123 | `items` | — | `media:length > 0` | — |
| Q124 | `items` | — | `@file.created:null = true` | — |
| Q125 | `pages` | — | `@collection.name = "pages"` | — |

### Q126–Q140: ICU ordering, pagination, and return modes

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q126 | `places` | — | `fictional = false` | `SORT BY name ASC` |
| Q127 | `places` | — | `fictional = false` | `SORT BY @file.size DESC, name ASC` |
| Q128 | `agents` | — | `corporate_body = false` | `SORT BY name ASC` |
| Q129 | `agents` | — | `corporate_body = false` | `SORT BY name ASC; OFFSET 50` |
| Q130 | `agents` | — | `corporate_body = false` | `SORT BY name ASC; OFFSET 500` |
| Q131 | `entries` | — | `year >= 1800 && year < 1850` | `SORT BY year ASC, preferred_title ASC` |
| Q132 | `entries` | — | `year >= 1800 && year < 1850` | `SORT BY year DESC, preferred_title ASC` |
| Q133 | `contents` | — | `entry = "00fx8ln651na0e3"` | `SORT BY numbering ASC, preferred_title ASC` |
| Q134 | `contents` | — | `numbering >= 0` | `SORT BY numbering ASC; LIMIT 100` |
| Q135 | `contents` | — | `numbering >= 0` | `SORT BY numbering ASC; LIMIT 100; OFFSET 1000` |
| Q136 | `contents` | — | `numbering >= 0` | `SORT BY numbering ASC; LIMIT 100; OFFSET 10000` |
| Q137 | `contents` | — | `entry = "00fx8ln651na0e3"` | `SORT BY numbering ASC; RETURN bytes` |
| Q138 | `contents` | — | `entry = "00fx8ln651na0e3"` | `SORT BY numbering ASC; RETURN logical` |
| Q139 | `contents` | — | `entry = "00fx8ln651na0e3"` | `SORT BY numbering ASC` |
| Q140 | `series` | — | `entries:length >= 3` | `SORT BY title DESC` |

### Q141–Q150: Combined FTS and FILTER requests

| ID | Collection | FTS | FILTER | Options overriding defaults |
|---|---|---|---|---|
| Q141 | `entries` | `"Almanach"` | `year >= 1800 && year < 1850` | `SORT BY year ASC` |
| Q142 | `entries` | `"Wien"` | `year = 1808` | — |
| Q143 | `entries` | `"Taschenbuch"` | `language ?= "ger"` | `SORT BY preferred_title ASC` |
| Q144 | `contents` | `"Liebe"` | `year >= 1800 && year < 1850` | — |
| Q145 | `contents` | `"Lied"` | `contributors.type ?= "Autor:in"` | — |
| Q146 | `agents` | `"Goethe"` | `corporate_body = false` | — |
| Q147 | `agents` | `"Müller"` | `data:exists = true && data:null = false` | — |
| Q148 | `items` | `"Original"` | `basis = true` | — |
| Q149 | `series` | `"Almanach"` | `entries:length >= 3 && @file.path ~ "data/series/"` | — |
| Q150 | `entries` | `"Almanach"` | `(edit_state = "Edited" \|\| edit_state = "Review") && year >= 1800 && !(places:length = 0) && @repository.root:exists = true` | `SORT BY preferred_title ASC` |

## References

These are component and syntax references, not claims that this engine inherits another product's entire behavior.

- [PocketBase operators and modifiers](https://pocketbase.io/docs/api-rules-and-filters/) — FILTER surface and existential/universal comparison conventions.
- [Rust BTreeMap](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html) — ordered posting dictionaries.
- [ICU4X collator and sort keys](https://docs.rs/icu_collator/latest/icu_collator/struct.CollatorBorrowed.html) — locale-aware sorting and key validity.
- [ICU4X case mapping](https://docs.rs/icu_casemap/latest/icu_casemap/struct.CaseMapper.html) — Unicode case operations.
- [time](https://docs.rs/time/latest/time/) — calendar dates and timestamp handling.
- [RoaringBitmap](https://docs.rs/roaring/latest/roaring/bitmap/struct.RoaringBitmap.html), [memchr](https://docs.rs/memchr/latest/memchr/), and [Rayon](https://docs.rs/rayon/latest/rayon/) — record sets, substring matching, and parallel execution.
- [ignore](https://docs.rs/ignore/latest/ignore/) and [notify](https://docs.rs/notify/latest/notify/) — file discovery and change hints.
- [arc-swap](https://docs.rs/arc-swap/latest/arc_swap/), [postcard](https://docs.rs/postcard/latest/postcard/), and [proptest](https://docs.rs/proptest/latest/proptest/) — publication, serialization, and generated tests.

## 22. Implementation phases and commit plan

This is the execution schedule for the implementing agent. Section 19 summarizes the capabilities; this section defines their implementation order, commit boundaries, and acceptance checks. Complete the phases in order. Where existing code already satisfies a step, integrate and validate it rather than rewriting it merely to create a commit.

### Working and commit procedure

1. Inspect repository instructions, the current branch/worktree, the configured remote, existing benchmark recordings, and the actual fixture schemas. The previous project is under `old/`, its core is `old/crates/core/`, the new core belongs in `core/`, and `examples/` remains unchanged at the root. The user performs the move into `old/`.
2. Create a separate branch such as `development/local-file-search-core` before implementation. Preserve unrelated changes and the archived project. Do not merge, force-push, or rewrite already pushed commits as part of this assignment.
3. Treat each numbered commit below as one reviewable implementation unit. Include its relevant tests and documentation in that same commit. Every committed state must build; do not leave intentionally failing tests or disconnected implementation fragments for a later phase.
4. Run formatting, the affected new-core tests, and the relevant CLI/integration checks before each commit. Run the broader new-workspace checks at phase boundaries. Avoid repeatedly rebuilding or testing all of `old/` unless a reused component actually requires it.
5. Inspect the diff, stage only the intended paths, commit with a precise message, then push that commit to the configured remote before beginning the next unit. Set the upstream on the first push. Preserve separate commits; do not combine an entire phase into one final submission.
6. If a step is too large to remain reviewable, split it into additional coherent commits and record the split. Do not create empty commits for already satisfied or inapplicable steps. If a push fails, record and report the failure accurately; do not claim it succeeded or rewrite history to conceal it.
7. Maintain a concise progress record, for example `docs/core-implementation-progress.md`, containing completed step IDs, validation commands/results, measured code revisions, benchmark report locations, reuse decisions, and remaining limitations. Reference completed commit hashes in subsequent progress/report commits; do not amend repeatedly just to make a document contain its own commit hash.
8. Development checks use small fixtures and selected workload cases. The final acceptance run executes all 150 cases in all four profiles. Reuse applicable current-state baselines; collect a new baseline only when none applies. Never report a disabled, unsupported, or unmeasured feature as a successful zero-time result.

The commit messages below are suggested subjects. Adapt their wording to the final changes while preserving their scope. No phase implements runtime file updates, update ingestion, watchers, or compaction. Future whole-file replacement is addressed through the required inventory and a design note.

### Phase 1 — Establish the new workspace and measurement contract

**Outcome:** A buildable new workspace, fixed workload inputs, and a reproducible record of implementation progress.

#### C01 — New workspace and core crate

Commit message: `build(core): establish the new search workspace`

Create the new Cargo workspace and `core/` crate, establish normal test/example targets, and preserve `old/` and `examples/`. Review any useful existing code freely and document the initial reuse decisions. Keep archived workspace members out of the default new-workspace build unless deliberately integrated.

**Before committing:** Build and test the minimal new workspace; verify fixture paths resolve from the repository root and no archived or fixture files were unintentionally changed.

#### C02 — Fixed workload and benchmark report definitions

Commit message: `test(bench): define the Musenalm workload and measurement records`

Commit the exact Q001–Q150 machine-readable manifest and definitions for timings, work counters, profile settings, run identity, and report files. Establish the progress record and identify any applicable saved baselines. Implement manifest loading and report serialization independently of executing queries.

**Before committing:** Validate all 150 unique IDs, inputs/options, the four profile definitions, and report round trips. Schema/semantic checks that require later query components remain explicit pending checks; do not emit invented measurements.

### Phase 2 — Logical records, source ownership, and extraction

**Outcome:** Source files can produce independently addressable logical records with complete ownership mappings.

#### C03 — Logical values, identity, and metadata

Commit message: `feat(core): implement logical values and record identity`

Implement strings, precise numbers, booleans, distinct date/datetime/unixtime types, nullability, homogeneous arrays, and nested objects. Define collection/file/record identities, typed field paths, and the reserved `@` metadata namespace.

**Before committing:** Test numeric precision, temporal distinctions, nullable/empty arrays, duplicate IDs, nested paths, and quoted names that collide with metadata names.

#### C04 — Repository inventories and file-to-record index

Commit message: `feat(core): add repository inventories and file record mappings`

Implement repository/collection ownership, generation-scoped record slots, source revisions, checked byte ranges, the explicit file-to-record index, and reverse record-to-file/range lookup. Start with small controlled inventory fixtures; no runtime updater is introduced.

**Before committing:** Verify both mapping directions, zero/one/multiple records per file, a file included in multiple collections, range/slot bounds, and per-file enumeration without a full record-inventory scan.

#### C05 — Source readers and fused extraction interface

Commit message: `feat(core): add source framing and extraction interfaces`

Define internal reader/scanner/value/text registrations and the build-time extraction interface. Implement the needed JSON support in the integration layer and small multi-record reference readers. Source framing must produce standalone ranges; extracted values and text segments must retain record/field boundaries.

**Before committing:** Parse each stored range independently, including non-ASCII prefixes and framing separators. Instrument one construction parse per record and verify malformed-source diagnostics and empty-file handling.

### Phase 3 — Query semantics and a runnable reference path

**Outcome:** Queries have a precise meaning and can execute correctly without index acceleration.

#### C06 — Separate FTS and FILTER parsers

Commit message: `feat(query): parse FTS and structured filter requests`

Implement literal substring FTS inputs and the FILTER grammar, including Boolean precedence, parentheses, negation, PocketBase comparison operators, quantifiers, modifiers, and metadata selectors. Represent sorting, COUNT, pagination, return mode, and evidence as query options.

**Before committing:** Test precedence, escapes, invalid syntax, standard comparison spellings, existential negation, and separate FTS/FILTER composition. Do not introduce word-tokenized FTS semantics.

#### C07 — Typed binding and independent reference evaluation

Commit message: `feat(query): bind typed predicates and add reference evaluation`

Resolve types, bindings, literal preparation, custom operation semantics, and defined modifier combinations. Implement an independent scan/reference evaluator for predicate membership and evidence, with no dependency on optimized index plans.

**Before committing:** Verify missing/null/empty distinctions, universal versus existential arrays, nested-element semantics, custom parsing/folding, and deterministic query errors. Test that query literals and selectors are prepared once rather than per record.

#### C08 — Musenalm schema registration and CLI execution

Commit message: `feat(cli): register the Musenalm repository and execute queries`

Read the fixture's actual schemas and register its collections, identity policies, types, source readers, and requested indexes/sort definitions. Connect the new core to the test CLI for individual requests and selected manifest cases. Preserve schema-declared strings and fractional numbers.

**Before committing:** Open the unchanged fixture, report actual file/record counts and registrations, and execute representative supported queries through the reference path. Unsupported later-stage options must produce explicit diagnostics.

### Phase 4 — Fused index construction and exact posting execution

**Outcome:** Typed lookups reduce or eliminate source verification, with measurable construction and execution work.

#### C09 — Fused builders and typed B-tree postings

Commit message: `feat(index): build typed postings in one extraction pass`

Construct B-tree value postings, presence/null entries, configured derived values, coverage, and compact statistics through one shared extraction pass. Reuse extracted values instead of reparsing a source separately for each index family.

**Before committing:** Compare equality/range/presence/null/cardinality results with the reference evaluator. Verify array deduplication preserves original length, incomplete coverage stays explicit, and construction parse counts remain one per record.

#### C10 — Exact access paths and restricted residual execution

Commit message: `feat(query): execute exact lookups before residual verification`

Add identity/metadata evaluation, typed posting lookup/ranges, exact-result tracking, candidate restrictions, and fallback scanning. Introduce basic cardinality-aware ordering immediately: a selective exact predicate must restrict a later scan regardless of written order.

**Before committing:** Verify zero verification reads for fully index-proven locator results, no source access for a fully covered empty conjunction, and correct uncovered-record/OR fallback. Compare results with the independent evaluator.

### Phase 5 — Bigram filtering and custom search paths

**Outcome:** Both repository alphabets and custom bindings provide conservative candidates with equivalent exact answers.

#### C11 — ASCII bigrams and default record text

Commit message: `feat(index): integrate ASCII record bigram filtering`

Integrate or adapt the existing ASCII bigram implementation, including supported normal/skip behavior. Build the collection's `raw` filter from extracted record text, preserve segment boundaries, and connect candidates to the prepared scanner.

**Before committing:** Verify substring matches, punctuation/whitespace, short strings, record boundaries, unsupported-character fallback, and absence of false negatives. Account for bigram construction in the shared parse budget.

#### C12 — Unicode bigrams and repository alphabet selection

Commit message: `feat(index): add Unicode bigrams and alphabet configuration`

Integrate the Unicode variant and repository-level `ascii|unicode` selection with consistent inheritance across default/custom filters. Record variant/configuration identity for later persistence and cache keys.

**Before committing:** Compare exact answers across alphabets, including non-ASCII queries, short queries, and mixed text. Confirm that normally only the selected variant is built and its memory cost is reported.

#### C13 — Custom bindings and combined text/filter execution

Commit message: `feat(query): connect custom text bindings to indexes and scanners`

Connect custom literal parsing, extraction/transformation, posting/bigram selection, and exact verification as one bound operation. Add the declared `agents.name:folded` CLI binding and combine FTS/FILTER restrictions in the shared executor.

**Before committing:** Exercise indexed and scanner-only variants of the same custom binding, selected FTS fields, coverage gaps, and the Q070 registration. Confirm that an equality posting cannot incorrectly become the complete answer to substring FTS.

### Phase 6 — Ordering, output, and the first complete baseline

**Outcome:** The complete workload's semantics are executable, including ICU sorting, COUNT, pagination, and all payload modes.

#### C14 — ICU keys and typed sorting lists

Commit message: `feat(index): add ICU and typed sorting lists`

Build configured ICU string keys/lists and typed numeric/temporal ordering, recording locale/options/version. Implement deterministic ties, descending order, and multiple sort fields.

**Before committing:** Compare ordering with a straightforward reference sort using the same collation contract. Test numeric precision, temporal ordering, equal collation keys, and stable pagination without source parsing solely for indexed sort keys.

#### C15 — Record results and grouped source access

Commit message: `feat(query): complete record results and grouped source reads`

Complete record COUNT before pagination, bytes/logical/locator outputs, file metadata, and requested matched-field evidence. Group source access by file, coalesce useful ranges, and reuse one decoded record for all residual predicates and logical output.

**Before committing:** Check all output modes and multi-record files, including sparse/dense candidates, exact membership versus payload loading, and at most one candidate parse per query. Verify that COUNT is never a file count or page length.

#### C16 — Runnable workload reports and baseline checkpoint

Commit message: `test(bench): establish the first complete query and build checkpoint`

Connect real execution to the measurement/report definitions and validate all manifest requests against the actual registrations. Reuse an applicable baseline; if none exists, record the first correct query/index-build baseline for supported profiles, normally the two alphabets with verified-result caching disabled at this stage.

**Before committing:** Save actual timings, raw samples, work counters, build measurements, corpus/configuration identity, and the measured code revision. Record unsupported cache/restore capabilities as pending, not simulated. Preserve this runnable revision for later comparisons. The baseline is not final acceptance.

### Phase 7 — Cost-based planning and execution refinement

**Outcome:** The engine chooses and reorders access paths by total work, with evidence that source scanning is reduced.

#### C17 — Access-path costs and maintained statistics

Commit message: `feat(query): select access paths using cost and selectivity`

Implement bound predicates, compatible access-path enumeration, inexpensive index statistics, cost/cardinality estimates, and collection-specific plan selection. Compare exact postings, bigrams, existing value/sort structures, dictionary evaluation, and restricted scanning without changing logical types.

**Before committing:** Use controlled fixtures to verify selective versus broad choices, precision-preserving typed operations, partial coverage, and custom-binding compatibility. Confirm planning does not scan source files or traverse whole dictionaries merely to estimate costs.

#### C18 — Boolean reordering and bounded adaptation

Commit message: `perf(query): reorder predicates and adapt to candidate counts`

Optimize AND/OR order, push candidate restrictions into remaining work, short-circuit exact decisions, and adjust remaining choices at plan/batch boundaries using observed cardinalities. Preserve negation, array witnesses, and evidence semantics.

**Before committing:** Run permutation/reference tests and deterministic work-count checks. Demonstrate that a cheap selective lookup precedes an expensive written-first term, a rare bigram can precede a broad posting, and an unindexed OR branch remains searchable.

#### C19 — Efficient set operations and plan diagnostics

Commit message: `perf(query): reduce intermediate work and expose execution plans`

Use appropriate sparse/dense representations, borrowed posting sets, reusable scratch buffers, and economical residual ordering. Complete EXPLAIN/EXPLAIN ANALYZE with chosen/rejected paths, execution order, estimates, actual work, and adaptive changes.

**Before committing:** Measure planning overhead and selected uncached queries against the retained checkpoint. Verify avoided allocations/source access where claimed, unchanged answers, and complete diagnostic serialization outside primary latency samples. This phase must finish before result caching is used to demonstrate query speed.

### Phase 8 — Repeated-query reuse and source consistency

**Outcome:** Repeated work is reused with correct generation boundaries; future mutation remains design-only.

#### C20 — Prepared-query reuse

Commit message: `perf(query): cache prepared predicates and plans`

Add bounded reuse of parsed/bound/prepared operations with semantic, binding, configuration, and generation compatibility. Avoid repeating literal preparation, registry resolution, and scanner initialization for identical requests.

**Before committing:** Verify prepared-plan hits, invalidation/rejection on incompatible state, memory accounting, and that plan-cache status is separate from verified-result-cache status. Include preparation in total query timing.

#### C21 — Verified membership and ordered-result caches

Commit message: `perf(query): cache verified matches and ordered results`

Add bounded verified membership/evidence and complete ordered-result reuse, independent of source-content caching. Support disabled and 64 MiB configurations, cached empty results, admission/eviction, and pagination across reused sequences.

**Before committing:** Verify no per-hit collection-wide stat/read pass, no approximate or partial result stored as complete, valid COUNT/pagination, and honest hit/miss reporting. Exercise both cache configurations with source/decoded caches disabled; record the first valid warm-cache measurements if no compatible recording exists.

#### C22 — Stale-source safeguards and future whole-file design

Commit message: `fix(core): enforce source revisions and document file replacement boundaries`

Complete source-access revision checks, explicit stale/incomplete outcomes, generation/cache isolation, and bounds validation. Document how future complete-file replacement will use the existing file-to-record index to identify and replace all old records.

**Before committing:** Test source changes during reads, invalid old offsets, incomplete-result cache rejection, and mapping consistency. Confirm that no runtime update ingestion, watcher, refresh service, or compaction mechanism was added.

### Phase 9 — Persistence and restoration

**Outcome:** A compatible saved snapshot restores the complete searchable state, including both inventory mappings.

#### C23 — Complete versioned snapshot encoding

Commit message: `feat(storage): persist inventories and all search indexes`

Serialize the logical/configuration envelope, file-to-record and record-to-file mappings, revisions, typed postings and special entries, selected bigram variant, ICU sorting state, coverage, and planner statistics. Implement atomic writes and complete decode/validation.

**Before committing:** Round-trip both alphabet variants and multi-record inventories; reject truncated/corrupt/incompatible sections, mismatched alphabets, invalid ranges, and inconsistent ownership. Do not serialize transient addresses or process-specific state.

#### C24 — Startup restoration and restore baseline

Commit message: `feat(storage): restore validated snapshots at repository startup`

Wire snapshot loading into repository open, reconcile current file metadata, and rebuild stale affected collections before publication. Unchanged compatible snapshots must reuse stored indexes. This startup behavior does not introduce live updating.

**Before committing:** Compare fresh-build and restored answers, ordering, COUNT, and mappings. Test reopening after file additions/replacements/deletions and missing/corrupt snapshots. If no applicable restore baseline exists, record the first correct fresh-process restore baseline, including full startup-to-query-ready time and memory for both alphabets.

### Phase 10 — Acceptance, measured improvements, and delivery

**Outcome:** A reviewable, pushed implementation with complete correctness evidence and reproducible final timings.

#### C25 — Cross-component acceptance validation

Commit message: `test(core): verify end-to-end search and restoration contracts`

Close uncovered integration cases across parsing, bindings, index coverage, optimization, payloads, caches, persistence, metadata, and multi-record source access. Keep the reference evaluator independent. Confirm all 150 requests are executable with their exact defined options.

**Before committing:** Run the complete relevant new-workspace correctness checks and CLI smoke tests. Include fixes with their regression evidence in separate focused commits when needed; do not hide unrelated changes inside a large test commit.

#### C26 — Evidence-driven performance corrections, only if needed

Suggested commit message: `perf(core): reduce <specific measured source of cost>`

Use existing profiles/plans and the applicable baselines to identify remaining allocation, copying, planning, parsing, index-build, or restore costs. Make one separately validated and pushed commit per justified correction. Replace the placeholder with the actual operation improved.

**Before committing:** Preserve correctness and compare the affected measurements using equivalent conditions, reporting absolute time and variability. Recheck build/restore/memory when those paths are affected. If no justified correction exists, record that this step was unnecessary and proceed without an empty commit. Do not implement deferred features to fill this step.

#### C27 — Final benchmark artifacts

Commit message: `test(bench): record final query build and restore measurements`

On the final validated code revision, execute all 150 fixed queries under ASCII/cache-disabled, ASCII/64 MiB-warm, Unicode/cache-disabled, and Unicode/64 MiB-warm profiles. Use the specified repetitions, include COUNT/page/payload completion, and measure construction and fresh-process restoration for both alphabets.

**Before committing:** Save all required manifests, raw samples, query tables, representative plans, memory/build/restore results, failures, and reproduction commands. Print all 150 timing summaries for every profile and the build/restore summaries. Compare with applicable baselines; lead with absolute timings. Each report must name the actual code revision measured. Keep large generated indexes and source copies out of the report commit.

Do not rebaseline simply because results are unfavorable. If a code fix is needed after this run, commit/push the fix separately and regenerate the affected final reports; keep the final measurement set consistent with the delivered code. Report-only commits do not require rerunning unchanged executable code.

#### C28 — Completion documentation and handoff

Commit message: `docs(core): document the completed search implementation`

Document build/run commands, actual architecture and reuse, grammar/types, registrations, optimizer behavior, inventory/snapshot formats, cache controls, freshness limitations, and the deferred whole-file update design. Complete the progress ledger with commit references and links to the saved measurements.

**Before committing:** Check that documented commands and paths match the delivered workspace, every required step is complete or accurately explained, all required commits are pushed, and the final report is tied to the tested code. Report remaining limitations plainly. Do not merge automatically.

### Final delivery checks

- The implementation runs from the new workspace and `core/`; `old/crates/core/` was available without artificial access restrictions, and root-level `examples/` is unchanged.
- File-to-record and record-to-file/range mappings are present, consistent, persisted, and validated. Runtime update handling remains unimplemented.
- Typed postings, ICU sorting, ASCII/Unicode bigrams, custom bindings, both query inputs, output modes, COUNT, and pagination work together.
- The planner selects compatible access paths and reorders work by cost/selectivity, demonstrated with uncached execution counters and saved plans.
- The final four-profile 150-query report, raw samples, build/restore measurements, and before/after evidence are saved and the full summaries have been printed.
- The branch contains separate validated commits with recorded push status. The user receives the branch, final commit, report locations, reproducible commands, and any outstanding limitations.

