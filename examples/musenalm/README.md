# Musenalm

A complete JSON import of the XML export supplied in `import/`, configured for the
Rust editor. The six collections have individual schema, settings and UI files in
`.editor/collections/` and one JSON file per record in `data/`.

| Collection | Records |
| --- | ---: |
| Series | 1,202 |
| Entries | 4,928 |
| Items | 4,180 |
| Contents | 133,948 |
| Agents | 4,010 |
| Places | 210 |

## Relationships

- `series.entries` holds the 4,908 **primary** series memberships formerly stored
  in `entries.series`. Twenty entries had no primary series in the source.
- `series.related_entries` embeds the 120 additional memberships from
  `R_entries_series`, including their annotation, timestamps and source row ID.
  These are separate from primary membership to preserve the source distinction.
- `entries.items` and `entries.contents` hold child IDs; child records no longer
  repeat their parent. Contents are ordered by their original `numbering`.
- `entries.agents` and `contents.agents` embed all 85,141 agent relationships,
  including role (`type`), conjecture, uncertainty, annotations and source row ID.
- `entries.places` retains the original array of references.
- Every actual reference has `x-relation` metadata, including nested agent and
  additional-series references, so normal validation and automatic ID renaming apply.

There are no junction collections or junction files. Original record IDs are the
JSON filenames; new records can use the editor's generated readable IDs.

## Source preservation

Text, inline HTML, timestamps and identifiers are preserved. Numbers, booleans,
arrays and arbitrary JSON retain their source types; JSON null is not an empty
string. Rich text uses Markdown controls, which also accept the original HTML.
The `editor` values refer to users absent from this export and remain read-only
source IDs. Scan filenames are retained, but the export did not include the binary
files. Created/updated timestamps and junction source IDs are read-only provenance.

`import-report.json` records counts and SHA-256 hashes of all source files. The
original XML remains local in the ignored `import/` directory. To reproduce the
import into a **new** directory:

```sh
python3 scripts/import-musenalm.py import /tmp/musenalm-reimport
cargo run --offline -p structured-editor-core --example validate_project -- /tmp/musenalm-reimport
python3 scripts/test-import-musenalm.py
```

The importer reads field definitions from `_collections.xml`, checks all supplied
references before writing, and refuses to overwrite existing projects.

The sidebar uses `sidebarTree: false`: it shows six collection links rather than
building the complete 148,478-record hierarchy on every page. Parent records still
show their child tables, with scoped browsing and column controls.
