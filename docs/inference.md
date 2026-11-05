# On-demand field inference

`Repository::infer_and_auto_index(collection, options)` explicitly rescans native JSON picker files in an already open repository. It does not infer fields from a collection currently containing selected custom-provider records. Pass `Some("books")` for one collection or `None` for all. `InferOptions::default()` builds ordered postings for inferred booleans, numbers, and low-cardinality strings; set `auto_index: false` for a report only. Inferred paths include object children and array elements. The call returns per-collection files, parse failures, observed paths, types, semantic hints, and posting status. Malformed JSON is counted but contributes no field observations. Opening a repository never triggers this scan.

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

In the TUI, choose **Infer fields and index rules** in the action palette (Ctrl+K). In the first prompt, **C** switches between the selected collection and all collections, **A** toggles advanced indexing, and **Enter** starts the background scan. When it finishes, a result panel shows the counts and offers **W** to add the discovered types and index rules to existing schemas. The write checks source and schema snapshots before changing files. **Esc** returns to the search. In-memory postings remain process-local until their rules are saved. Repeated inference rebuilds inferred postings from the current source.

For a one-shot JSON report, run `objekte REPOSITORY inferschema [COLLECTION]`. To scan and save inferred types and index rules in existing schema files, run `objekte REPOSITORY writeschema [COLLECTION]`. Both accept `--collection NAME` instead of a positional collection, search all collections when none is selected, and include `schema_written` per collection in the report. `--no-auto-index` infers types without recommending or saving ordered posting rules; it leaves existing index settings alone. `writeschema` requires at least one existing schema in its scope. These commands also accept `--diagnostics` for pretty JSON timing and log output on stderr.

## Detection rules

- Boolean, integer, number, string, object, array, null, and mixed are observed from parsed JSON. Null and missing are counted separately. Integer means all observed non-null numbers use integer syntax. A path with mixed non-null kinds stays mixed and gets no inferred typed posting.
- An enum candidate has at least 20 nonblank string values across 10 files with nonblank values. Empty and whitespace-only strings do not count toward its repetition or distinct-value thresholds, so a blank-only field remains a string. Every value is at most 64 bytes; there are at most 64 distinct values using at most 8 KiB of distinct text. At least 80% of nonblank observations repeat, and the nonblank distinct count is at most one tenth of nonblank observations (with a floor of four). A field that exceeds the bounds drops its provisional enum posting to limit memory. A path with more than 500,000 posting rows or more than 4,096 scalar values in one file also drops its provisional posting. When saving, inferred enum values are written as `x-enum-suggested`, without blanks. The TUI offers them as filter completions, but validation does not enforce them: a sample cannot prove that a set of values is closed.
- A record link requires at least 10 nonempty values in at least five files, and at least 95% must resolve to IDs unique across repository collections and point to the same collection. A JSON Pointer candidate must use `#/...` or a relative `.json#/...` address and resolve to a real target in a JSON file. Pointer links are reported but are not written as `x-relation`, because that schema keyword currently describes record IDs only.
- HTML needs recognized HTML tags; XML needs a parseable element structure, with XML declarations and namespaces taking priority; Markdown needs parser-recognized structure such as headings, lists, links, or emphasis. At least three bounded samples and 20% of sampled values must vote for the winning format, with twice the votes of the runner-up. At most 512 strings of at most 64 KiB each are classified per field. Ambiguous strings stay plain.

## Schema writing

After a scan, call `Repository::write_inferred_schema(&mut report)` to update a collection's **existing** schema file without reparsing JSON. `InferOptions { write_schema: true, .. }` performs this step immediately after the scan. Saving only ever adds to a schema; it never removes or narrows what is declared.

- **New fields:** an observed field the schema does not declare is added with its type and observed nullability. Numbers are saved as `number`, because whole numbers in a sample do not prove that decimals cannot occur. A suggested enum, record link (`x-relation`), and text format (`x-full-text`) are added too. Fields that were always null, and mixed fields, are not declared.
- **Declared fields:** the declared type, enum, relation, text format, structure (`properties`, `required`, `items`), and other settings stay as written, even when the data disagrees. The type is only widened: it gains `null` when nulls were seen, and `integer` becomes `number` when decimals were seen. A declared enum then also admits `null`. A declared string without an enum gains `x-enum-suggested` when its values look like an enum. Records that contradict a declaration stay invalid until you change the schema.
- **Index rules:** with advanced indexing enabled, a recommended top-level field gains `index: true` unless it already has an `index` setting; an explicit `index: false` is kept. Recommended nested paths are added to `x-index-paths`. Declared index settings and paths are never removed. With advanced indexing off, saving adds types only.

Other discovered posting paths are saved in the root `x-index-paths` array, using filter path syntax, for example `"detail.rank"` and `"items[*].score"`. A later repository opening builds all those paths in one additional source pass, sharing each file read and parse among the paths. This list creates query postings, not sortable view keys. Nested array and object type declarations are added where the schema permits them. The updated schema is compiled before an atomic replacement. `write_inferred_schema` and `InferOptions { write_schema: true, .. }` leave the current repository handle bound to its previously loaded schema; call `reload()` or `refresh_configuration()` before querying with the new rules. Schema-free collections stay schema-free.
