# Schema-driven child lists

Orte and Sammlungen are editable records as well as containers. Their child lists are part of the data schema, independent of the server UI:

```json
{
  "sammlungen": {
    "type": "array",
    "uniqueItems": true,
    "default": [],
    "x-children": true,
    "items": { "type": "string", "x-relation": { "collection": "sammlungen" } }
  },
  "handschriften": {
    "type": "array",
    "uniqueItems": true,
    "default": [],
    "x-children": true,
    "items": { "type": "string", "x-relation": { "collection": "handschriften" } }
  }
}
```

These are top-level `properties` of an Ort schema. Sammlung declares the same `handschriften` property. A parent JSON record stores arrays of IDs, alongside its own `name`, `description`, etc. Manuscript JSON contains the manuscript fields, with no stored `Ort` or `Sammlung`. This permits shared placements without copying content. A record with no parent remains accessible in its collection and in the link-existing picker.

The core discovers child-list definitions, enforces existing targets and unique membership, rejects collection cycles, queries parent links, and returns a hierarchy. Renaming/deleting referenced children retains the existing incoming-reference guard. Removing membership leaves the child intact. Parent revisions guard every membership change. Contextual creation checks the parent revision before creating and rolls back the new child if attachment fails. This is not a cross-process/crash-atomic multi-file transaction: a process interruption between writes can leave an unattached child, accessible from its collection.

The UI renders each child list independently next to the parent's metadata. `Browse list` opens the standard searchable, sortable, filterable table scoped to that parent. Its URL includes `parent_collection`, `parent_id`, and `parent_field`; these always remain server-validated. New child sessions retain that context, and successful saves return to the parent overview. Raw JSON still exposes child arrays; the ordinary metadata form preserves them and leaves membership management to the lists.

Collection `ui.json` controls:

- `fieldOrder`: ordering of metadata and child sections.
- `icon`: Remix Icon for records in this collection.
- `singularLabel`: contextual creation label.
- `sidebarLabel` and optional `sidebarIcon`: root navigation presentation.
- `allCollection`: optional collection opened by the special root *All* entry.
- `list.columns`, `list.defaultSort`, and `list.filters`: existing table settings, also used for child lists.

The example exposes Orte as the **Handschriften** root. Clicking its label opens Orte; the chevron expands/collapses the tree. Its *All* entry opens every manuscript, including unattached ones. Clicking an Ort or Sammlung opens its record and expands its children; each branch's *All* opens that parent overview. Labels and chevrons are separate targets, and branch and leaf labels use identical indentation at a given depth. Alphabetic lists, trees, facets, suggestions, and media names use Unicode collation (case-insensitive, accents at secondary strength; canonically equivalent strings compare equally). Numeric fields and child counts sort numerically. Stable IDs break collation ties.

## Lenz migration

`scripts/migrate-lenz-hierarchy.py [project] --apply` migrates the old child-side references. Without `--apply` it only checks the conversion. It verifies IDs and all non-placement values, writes an original-JSON backup to a temporary directory, and refuses mixed migration states. A second run is a no-op. Schema/UI files must be updated with the migration, and the server restarted.

The checked-in example had **41 Orte, 62 Sammlungen, and 678 manuscripts**: 611 manuscripts under Sammlungen and 67 directly under Orte. All redundant Ort references agreed with their Sammlung. The migration preserved existing IDs, descriptions, Markdown, and source metadata. Import/conversion scripts now emit the same parent-owned structure. The older Astro frontend does not implement these new container pages; the Rust server is the active UI.
