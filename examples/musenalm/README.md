# Musenalm

The real-data fixture for the first Objekte implementation profile. All seven JSON
collections are registered in `.editor/repository.json`. Each has a minimal
structural schema, collection settings, and scalar list options under
`.editor/collections/`. The implementation contract is in
[implementation.md](../../implementation.md).

| Collection | Records | Initial use |
| --- | ---: | --- |
| Places | 210 | First small default table; strings, numbers, booleans |
| Agents | 4,010 | Larger table, long strings, opaque JSON/null |
| Series | 1,202 | Reference arrays and embedded membership objects |
| Entries | 4,928 | Large nested records and nested references |
| Contents | 133,948 | Large collection; numeric and fractional sorting |
| Items | 4,180 | Arrays, booleans, owner filters |
| Pages | 8 | Small collection with opaque JSON |
| **Total** | **148,486** | |

All collections are allowed by the first data profile. Start end-to-end development
with `places`, then `agents`; use `series` and `entries` for relationships and
`contents` for scale. `contents.numbering` includes 1,038 fractional values, so
numeric sorting must support fractions from the beginning.

## Minimal schema and UI

Schemas retain properties, types, required fields, closed-object rules, arrays of
embedded objects, and explicit `x-relation` declarations. Opaque imported JSON uses
an empty schema. Rich validation, defaults, and presentation annotations have been
removed for the first profile. HTML, Markdown, dates, and URLs remain strings.

UI files specify a label, display field, scalar table columns, available filters,
default sort, and `sortable` fields. The library derives typed indexed fields from
these options and the schema. Filter controls are inactive until values are given.
The repository UI supplies a default page size of 50. Nested values remain available
through full record reads; the first table views do not expand them.

Collection settings contain only source type/path/format, filename identity, and
schema/UI paths. The former instance settings files remain as unregistered files;
named documents and richer editor settings are outside this first profile.

## Actual stored relationships

- `series.entries` contains primary references to entry files.
- `series.related_entries` embeds additional membership objects; their `entry`
  field references an entry file.
- `entries.places` contains references to place files.
- `entries.agents[].agent` and
  `entries.contents[].agents[].agent` reference agent files.
- `entries.items` and `entries.contents` contain **embedded objects**, not child
  IDs. Their `source_id` fields preserve provenance but declare no relation.
- Standalone `contents` and `items` files also exist. Their shapes differ from the
  embedded values; the library must not merge or synchronize them implicitly.
- Standalone `contents.contributors[].agents` references agent files.

The schema graph therefore links series to entries, entries to agents/places, and
standalone contents to agents. Items and pages have no declared outgoing relations.
`editor` IDs refer to users absent from this fixture and remain plain strings.

Identity is `(collection, filename stem)`. An embedded value has no independent
file identity. References inside it belong to its containing record.

## Verification and source preservation

On 28 October 2026, all 148,486 source records passed the simplified schemas using
a Draft 2020-12 validator. All 173,427 declared reference occurrences resolved to
existing target files; this counts occurrences, including embedded copies, not
distinct targets. All configured display/filter/sort/table fields resolve to scalar
schema fields. Source-file content hashes were identical before and after this
configuration change.

JSON values, inline HTML/Markdown, timestamps, identifiers, and filenames are
preserved. Null remains distinct from an empty string. The original XML export is
local in the ignored `import/` directory; `import-report.json` describes its import.
The existing import/migration scripts describe earlier export/editor stages and do
not reproduce this minimal configuration profile. The checked-in JSON data and
the current schema/UI files are the acceptance fixture for the rebuild.
