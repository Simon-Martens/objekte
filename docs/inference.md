# On-demand field inference

`Repository::infer_and_auto_index(collection, options)` explicitly rescans JSON source files in an already open repository. Pass `Some("books")` for one collection or `None` for all. `InferOptions::default()` builds ordered postings for inferred booleans, numbers, and low-cardinality strings; set `auto_index: false` for a report only. Inferred paths include object children and array elements. The call returns per-collection files, parse failures, observed paths, types, semantic hints, and posting status. Malformed JSON is counted but contributes no field observations. Opening a repository never triggers this scan.

```rust
use objekte_core::{InferOptions, Repository};

let mut repo = Repository::open("examples/musenalm")?;
let report = repo.infer_and_auto_index(Some("places"), InferOptions {
    auto_index: true,
    write_schema: false,
})?;
println!("{} paths indexed", report.collections[0].indexed_paths);
# Ok::<(), objekte_core::Error>(())
```

The TUI offers the operation with **Alt+I** or **Ctrl+I**; **i** also works from the results table. In the first prompt, **C** switches between the selected collection and all collections, **A** toggles advanced indexing, and **Enter** starts the background scan. When it finishes, a result panel shows the counts and offers **W** to write the discovered types and index rules to existing schemas. The write checks source and schema snapshots before changing files. **Esc** returns to the search. In-memory postings remain process-local until their rules are saved. Repeated inference rebuilds inferred postings from the current source.

For a one-shot JSON report, run `objekte REPOSITORY inferschema [COLLECTION]`. To scan and save inferred types and index rules in existing schema files, run `objekte REPOSITORY writeschema [COLLECTION]`. Both accept `--collection NAME` instead of a positional collection, search all collections when none is selected, and include `schema_written` per collection in the report. `--no-auto-index` infers types without recommending or saving ordered posting rules; it leaves existing index settings alone. `writeschema` requires at least one existing schema in its scope. These commands also accept `--diagnostics` for pretty JSON timing and log output on stderr.

## Detection rules

- Boolean, integer, number, string, object, array, null, and mixed are observed from parsed JSON. Null and missing are counted separately. Integer means all observed non-null numbers use integer syntax. A path with mixed non-null kinds stays mixed and gets no inferred typed posting; its existing schema type remains unchanged because the supported schema subset has no mixed type declaration.
- An enum candidate has at least 20 string values across 10 files. Every value is at most 64 bytes; there are at most 64 distinct values using at most 8 KiB of distinct text. At least 80% of observations repeat, and the distinct count is at most one tenth of observations (with a floor of four). A field that exceeds the bounds drops its provisional enum posting to limit memory. A path with more than 500,000 posting rows or more than 4,096 scalar values in one file also drops its provisional posting. Inferred enum values are written as a schema `enum` when saving rules. Future values outside that set require another scan or a schema edit.
- A record link requires at least 10 nonempty values in at least five files, and at least 95% must resolve to IDs unique across repository collections and point to the same collection. A JSON Pointer candidate must use `#/...` or a relative `.json#/...` address and resolve to a real target in a JSON file. Pointer links are reported but are not written as `x-relation`, because that schema keyword currently describes record IDs only.
- HTML needs recognized HTML tags; XML needs a parseable element structure, with XML declarations and namespaces taking priority; Markdown needs parser-recognized structure such as headings, lists, links, or emphasis. At least three bounded samples and 20% of sampled values must vote for the winning format, with twice the votes of the runner-up. At most 512 strings of at most 64 KiB each are classified per field. Ambiguous strings stay plain.

## Schema writing

After a scan, call `Repository::write_inferred_schema(&mut report)` to update a collection's **existing** schema file without reparsing JSON. `InferOptions { write_schema: true, .. }` performs this step immediately after the scan. Every observed field with a representable inferred type receives that type and observed nullability, even when the schema already declares it. Existing enum constraints are replaced by inferred enum values or removed when a field is no longer an enum. With advanced indexing enabled, the save also synchronizes each observed field's `index` setting and nested posting rule, including changing an existing `index: false` to `true` or an outdated `true` to `false`. With advanced indexing off, the save updates types and leaves index settings alone. Unobserved fields, root settings, and compatible field settings remain as they were. When a type changes, incompatible shape or string-only keywords are removed so the schema remains valid. Record links and text formats are added for newly declared fields.

Other discovered posting paths are saved in the root `x-index-paths` array, using filter path syntax, for example `"detail.rank"` and `"items[*].score"`. A later repository opening builds all those paths in one additional source pass, sharing each file read and parse among the paths. This list creates query postings, not sortable view keys. Nested array and object type declarations are added where the schema permits them. The updated schema is compiled before an atomic replacement, and the repository reloads changed schemas before returning. Schema-free collections stay schema-free.
