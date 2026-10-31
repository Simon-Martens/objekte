# Musenalm

The real-data fixture for the first Objekte implementation profile. All seven JSON
collections have named schemas under `.objekte/collections/`. Each schema's
`x-objekte` object declares its name, source directory, data type, JSON format,
and filename identity. A sample named view lives under `.objekte/views/`.
The implementation contract is in
[implementation.md](../../docs/implementation.md).

| Collection | Records | Initial use |
| --- | ---: | --- |
| Places | 210 | First small default table; strings, numbers, booleans |
| Agents | 4,010 | Larger table, long strings, opaque JSON/null |
| Series | 1,202 | Reference arrays and embedded membership objects |
| Entries | 4,928 | Entry metadata and references to agents and places |
| Contents | 133,948 | Each content references one entry; numeric and fractional sorting |
| Items | 4,180 | Arrays, booleans, owner filters |
| Pages | 8 | Small collection with opaque JSON |
| **Total** | **148,486** | |

All collections are allowed by the first data profile. Start end-to-end development
with `places`, then `agents`; use `series` and `entries` for relationships and
`contents` for scale. `contents.numbering` includes 1,038 fractional values, so
numeric sorting must support fractions from the beginning.

## Minimal schemas and independent views

Schemas retain properties, types, required fields, closed-object rules, arrays of
embedded objects, and explicit `x-relation` declarations. Opaque imported JSON uses
an empty schema. Rich validation, defaults, and presentation annotations have been
removed for the first profile. HTML, Markdown, dates, and URLs remain strings.

Collection schemas contain no view options. The separate `contents_list` view
defines a list type, `page` and `from_year` arguments, and a JSON query. The
coming UI can register its own default list, reference, and autocomplete views
for any collection without changing those schemas. Indexes come only from the
schema's `index` and `x-index-paths` declarations. Nested values remain available
through full record reads and queries.

## Actual stored relationships

- `series.entries` contains primary references to entry files.
- `series.related_entries` embeds additional membership objects; their `entry`
  field references an entry file.
- `entries.places` contains references to place files.
- `entries.agents[].agent` references agent files.
- `entries.items` contains embedded objects, not child IDs. Their `source_id`
  fields preserve provenance but declare no relation.
- Every standalone `contents.entry` references exactly one entry file. The
  content is stored only in that standalone file; entry files contain no
  `contents` list. The `entry` field has an ordered index for backlink searches.
- Standalone `items` files also exist. Their shapes differ from the embedded
  values; the library must not merge or synchronize them implicitly.
- Standalone `contents.contributors[].agents` references agent files.

The schema graph therefore links series to entries, entries to agents/places,
and contents to entries/agents. Items and pages have no declared outgoing relations.
`editor` IDs refer to users absent from this fixture and remain plain strings.

Identity is `(collection, filename stem)`. Embedded items and agents have no
independent file identity. References inside them belong to their containing record.

## Verification and source preservation

On 28 October 2026, before the content-to-entry migration, all 148,486 source
records passed the simplified schemas using a Draft 2020-12 validator. The
31 October migration removed the embedded `entries.contents` copies and added
one required `contents.entry` reference to each of the 133,948 standalone content
files. Every reference resolves to one of the 4,928 entry files; 2,394 entries
have at least one content. The full acceptance test validates all 148,486
post-migration records and confirms direct and indexed query paths agree.

Standalone content payloads, inline HTML/Markdown, timestamps, identifiers, and
filenames are preserved apart from the new `entry` field. Entry fields other than
the removed `contents` list are preserved. Null remains distinct from an empty
string. The original XML export is
local in the ignored `import/` directory; `import-report.json` describes its import.
Its `entries.contents` count describes the original embedded copies, which are
no longer stored in entry files.
The existing import/migration scripts describe earlier export/editor stages and do
not reproduce this minimal configuration profile. The checked-in JSON data and
the current named schemas and view file are the acceptance fixture for the rebuild.
