This revised MVP incorporates your comments. It defines the intended behavior for the rebuild; it does not prescribe frontend technology or architecture. Accepted implementation decisions are recorded below; configuration and module details are in README.md and crates/server/README.md.

**Governing UI principle: browse the data tree**

The UI is a schema-guided browser and editor of the JSON data tree. For every collection and every assigned schema, it presents objects through their properties, arrays through lists of their items, and nested objects through their own properties. The same model applies recursively: a collection list leads to an object, an object exposes properties and child lists, and each child can be explored in turn. Markdown/frontmatter records use their configured data mapping within this model.

Embedded objects and schema-declared child references share this browsing model, but retain their different storage semantics. Following a reference opens the existing record; it does not copy that record into the parent JSON. Relationship trees are views over those links, not a requirement to store all content in one physical JSON tree.

UI configuration customizes the presentation of this structure. Applicable options—such as labels, visible properties, property order, list columns, sorting, and expanded or compact display—must be configurable for an individual object, property, array/list, or item view, and collectively through shared defaults. Configuration must support explicit scopes for a node itself, its descendants, or a group of siblings under the same parent, as well as defaults for an entire collection/schema or the whole application. Inheritance is the default. A local override changes only the specified options; unrelated inherited options remain effective. Setting `inherit: false` on an option object discards its inherited options and uses local options plus built-in defaults. More specific settings take precedence over broader defaults, with deterministic conflict rules. Sibling scope is local to a parent view; it must not inadvertently change every use of the same target collection.

Presentation options do not change stored values, ownership, reference identity, or schema validation. This is a consistent way to browse and configure data-derived views, not arbitrary panel or screen composition. Implemented configuration syntax and precedence are documented in crates/server/README.md; broader descendant and sibling scopes must follow the same inheritance rule. Existing implementation limitations below are gaps toward this principle, not exceptions to it.

**1. Configuration scope**

Configuration is separated into repository-wide and collection-specific concerns. There is no single object schema containing all collections.

| Configuration | Responsibility |
|---|---|
| Repository configuration | Register collections and additional schema-to-file mappings; locate editor settings and global UI configuration |
| Editor-instance settings | Settings for this editor instance, separate from repository content |
| Global UI configuration | Overall navigation, sidebar groups, collection placement, and shared presentation defaults |
| Collection settings | Collection type, storage location, format, identity rules, and permitted operations |
| Collection schema | Fields, types, constraints, required properties, and explicit defaults for that collection |
| Collection UI configuration | Field controls, field order, labels, list/search presentation, trees, and visibility |
| Additional schemas | Validate one or multiple explicitly assigned JSON files anywhere in the repository |

Sidebar configuration is maintained **only through global and collection UI JSON files**. There is no sidebar-content customization screen or browser-local navigation configuration. Sidebar width and collapsed state are view preferences controlled beside the app title and remembered locally.

Arbitrary panel and screen composition is deferred.

**2. Collections and storage**

Two collection types are in scope:

| Collection type | Content | MVP behavior |
|---|---|---|
| Data | JSON or Markdown with YAML frontmatter | Schema-based reading, creation, editing, validation, saving, renaming, and deletion |
| Media | Files and directories | Browsing, uploading, folder creation, moving, and deletion |

Each collection has its own settings and UI configuration. Storage format, directory, identity rules, available operations, and presentation are configured per collection rather than assumed globally.

Data collections require:

- A collection-specific schema.
- Record identification and ID syntax/uniqueness checks.
- ID generation when requested.
- An explicit mapping between stored content and fields—for example, the Markdown body and YAML frontmatter.
- Validation before saving.
- Revision checks that reject overwriting externally changed content.
- Atomic file replacement.
- Preservation of fields not exposed in the normal editing view.

Additional schema-to-file mappings remain available for documents that do not need collection browsing. A mapping can govern one file or several files.

**3. Field types and controls**

The schema defines the stored values and their constraints. UI configuration selects how those values are edited and displayed.

| Field/control | Stored value and schema information | UI configuration and behavior |
|---|---|---|
| Text | String; applicable length and pattern constraints | Explicit single-line or multiline presentation |
| Integer | Integer; applicable bounds | Integer input; blank is not zero |
| Boolean | Boolean | True/false control; missing is distinct from false |
| Markdown | String | One Markdown control type, configured for inline or multiline presentation; source editing and preview |
| HTML | String containing HTML/template source | Source editing and read-only source display |
| Date | String with date constraints | Date input and display |
| Date and time | String with date-time constraints | Date/time input and display |
| Enum | Schema-defined set of allowed values | Selection from preset values; preserve the values’ JSON types |
| Record reference | Target collection and record ID | Search, select, resolve label, navigate to target |
| Media reference | Reference/path identifying a media item | Browse and select media |
| Array | Ordered values with an item schema | Add, edit, remove, and reorder items of **any supported type**, including nested arrays |
| JSON | Any JSON value, with a schema when available | Raw JSON editing; fallback when no applicable schema/control definition exists |
| System field | Reserved metadata with a defined system meaning | Usually generated or read-only; special behavior where appropriate |

Nested JSON objects should expose their properties recursively, including objects inside arrays, in accordance with the governing UI principle. Raw JSON is also an explicit presentation choice for any subtree and its contents, including a whole document. Nested object properties and homogeneous object arrays now use recursive subtree views and the document sidebar. Scalar arrays remain inline. Inherited `navigation: "stop"` makes an object an endpoint: scalar fields and scalar arrays stay structured, while deeper containers use explicit raw presentation or the configured fallback. `rawFallback: false` preserves unsupported values without exposing a raw editor; explicit `editor: "raw"` still works. Local overrides or `inherit: false` reset inherited options.

The JSON fallback still validates against any available schema. Without a schema, it checks JSON syntax.

Conditional form controls and table controls are deferred.

**4. Presence, required values, defaults, and ownership**

These are shared behaviors, not separate field types.

- Required and optional status comes from JSON Schema.
- Defaults come from JSON Schema **only when explicitly declared**.
- The editor must not invent empty strings, zero, false, or other values simply because a field exists.
- Optional properties may remain absent or be removed.
- Required properties must be present before a valid save.
- Absence, empty string, zero, false, and explicit null remain distinct. Null is accepted only where the schema permits it.
- Invalid or incomplete input may exist in an editing draft without silently becoming another value.

Every field also has an ownership policy:

| Ownership | Behavior |
|---|---|
| User-entered | Editable according to permissions |
| Generated | Supplied by a defined system operation |
| Imported | Preserved from imported content; editability explicitly defined |
| Immutable | Preserved but not changeable through normal editing |

Visibility and ownership are separate. A hidden field is not automatically generated, immutable, or optional.

The earlier “provenance fields” referred to imported metadata such as `source` and `source_index`. They are ordinary fields with an ownership policy, not a distinct field type.

**5. System fields**

System metadata uses reserved names or a reserved namespace so that ordinary user fields cannot accidentally acquire special behavior. The reserved namespace is `_editor`, with `schema_revision` and `manual_order`.

| System field | Purpose | MVP behavior |
|---|---|---|
| Schema revision | Identify the schema revision applicable to a particular record | Store a reference to the relevant schema version, using a commit hash |
| Manual order | Determine an explicitly assigned order among records | Optional ordering value; absent means no manual order has been assigned |

The schema revision identifies the applicable schema. It is separate from the file revision used to detect concurrent edits.

Manual order is a system concept, not special behavior inferred from an ordinary field named `Sort` or `order`. Views define how manually ordered and unordered records are presented.

Automatic schema migration is not part of this MVP.

**6. Relations and reference protection**

Record references require:

- A configured target collection.
- Target-ID validation and label resolution.
- Search and selection.
- Navigation to the referenced record.
- Optional creation of a target record from the selection control.
- Incoming-reference discovery.
- Blocking renaming or deleting referenced records for now.

For the Ort/Sammlung relationship:

- Orte own `sammlungen` and `handschriften` arrays; Sammlungen own `handschriften` arrays.
- Arrays reference stable IDs. Each record has one stored copy, even when included in multiple lists.
- A manuscript can belong directly to an Ort, to a Sammlung, or to both. There are no stored child-side Ort/Sammlung selections.
- Schema-declared child lists generate navigation and lists alongside the parent’s editable metadata. New, edit, link-existing, and remove-from-list actions preserve record identity.

Media behavior is deliberately narrower:

- Explicit media-reference fields can identify references and protect referenced media from destructive operations.
- **Media links inside Markdown are not tracked or used to block operations.**
- Media selection and Markdown link insertion are UI features.
- Validation of file existence, file type, and other file properties is deferred.

Record links inside Markdown remain within record-reference tracking; your requested removal concerned media links.

**7. Collection UI and navigation**

| Feature | Required configuration/data | MVP behavior |
|---|---|---|
| Collection navigation | Label, sidebar group, placement, visibility | Present configured collections |
| Collection lists | Display fields, labels, ordering | Browse records |
| Search | Searchable fields and search-result presentation | Search configured fields and IDs |
| Pagination | Result position and page size | Navigate larger result sets |
| Trees | Relationship/grouping definition, labels, counts, ordering | Navigate hierarchical views |
| Tree-node information | Description and underlying record | Display context and open the corresponding record |
| Contextual creation | Current location and relevant relationship values | Create a record with explicit contextual values |
| Record view | Visible fields, field order, control definitions | Preview and edit a record |
| Record actions | Permissions, identity, revision, references | Save, rename, delete, generate/check ID |
| Raw document view | Entire record and schema | Edit the document as JSON |
| Reference view | Referring records and field locations | Explain incoming references |
| Markdown link insertion | Selection, target search, URL, label, image option | Insert or edit links |

Tree structure is a view over relationships. It must not introduce additional data-validation rules merely because a tree is displayed.

Label definitions belong to view configuration. Lists, search results, record headings, and reference controls may have different configured labels, but should use the same label-generation behavior.

Suggestions remain separate from enums:

- **Enum:** only preset values are valid.
- **Suggestions:** offer existing or configured values while accepting new ones.

The MVP needs suggestions for fields such as `Werk` and `Sprache`, backed by distinct values and cache refresh after writes.

**8. Editor settings and additional documents**

- `settings.json` belongs to the editor instance.
- Its schema uses the same general schema-assignment mechanism as other JSON documents.
- Additional schemas may govern singular or multiple JSON files.
- Assigned documents can be opened, validated, and saved.
- Assigned documents share typed previews and the record draft editor, including array controls, validation, history, and conflict checks. Raw editing is an optional presentation mode; nested objects share recursive controls and navigation.
- Saving uses the same revision/conflict behavior as other document editing.
- Imports preserve editor settings, schema mappings, and existing assigned documents.

No concrete instance-setting options are specified yet.

**9. Application functionality**

These remain outside field schemas:

- Authentication and logout.
- Reader, editor, and administrator permissions.
- Account/password management.
- Invitations and password resets.
- User administration and session revocation.
- Shared loading, empty-state, error, and save-conflict behavior.

**10. Remaining inconsistencies and generalizations**

Review of the current Rust implementation against the governing UI principle:

| Area | Current implementation and next step |
|---|---|
| Schema-driven records and arrays | Record properties use shared field definitions for editing and preview; arrays recursively render item controls. Nested objects use the same controls and subtree navigation. |
| Nested objects | Objects and homogeneous object-array items support recursive property views, editing, and sidebar navigation. Explicit endpoints and raw subtrees stop expansion. |
| Shared UI options | Global defaults, collection UI, and global collection overrides now merge individual options recursively, with `inherit: false` resets. Presentation settings inherit into fields and array items; parent-owned child lists support shared sibling defaults and local overrides. Object views inherit presentation settings recursively; inheritance across reference navigation remains a separate design question. |
| Child lists | Each parent/list occurrence inherits target collection defaults, then parent-local shared and individual overrides. Preview and full-list views share the resolved columns, sorting, and filters; preview size is configurable and columns are not capped at four. Record identity is unchanged. |
| Assigned JSON documents | Mapped documents now share typed previews and the existing draft editor, including array/scalar roots, without record-ID actions. Global defaults and mapping-specific options select presentation, including raw JSON for a subtree or the whole document. Nested objects share recursive structured controls. |

Suggested next step: expose navigation stops and raw/fallback choices in a per-node view-options menu. Define any scopes across reference navigation explicitly rather than silently changing another occurrence of a shared record.

| Issue | Corrected direction or remaining decision |
|---|---|
| Required but hidden imported fields | Define their ownership and creation behavior. A new record must not require unexplained metadata that users cannot supply. |
| Inline Markdown currently changes escaping behavior | Inline/multiline becomes a UI choice. Switching presentation must not silently change stored content. Both modes edit Markdown source. |
| Inline Markdown currently rejects line breaks | Presentation alone must not impose this data constraint. Retain it only where explicitly required by the field schema. |
| Manuscripts can appear directly under an Ort or under a Sammlung | Parent-owned child arrays represent both placements. Sharing a manuscript across lists does not duplicate its content. |
| Schema revision is newly required | Use the latest commit containing the exact loaded schema. Existing records stay unchanged until saved. Warn and omit the stamp for uncommitted schemas. |
| Manual order is newly standardized | Order is scoped to a collection; default unordered placement is last, configurable per view. Ordinary field names have no implicit meaning. |
| Date-time representation is not yet specified | Store RFC 3339 with an explicit offset; preserve offset and entered precision. |
| Defaults and initialization are currently mixed | Separate schema defaults, contextual prefilling, generated metadata, and the user’s draft. |
| Labels are assembled in multiple places | Use shared label-generation behavior driven by each view’s UI configuration. |
| Field behavior is intertwined with controls | Generalize presence, ownership, validation feedback, read-only state, and array operations across controls. |
| Media links currently participate in reference tracking | Remove Markdown-media tracking from the target MVP; keep explicit references separate. |
| Sidebar preferences are currently local | Replace them with configuration-only global and collection UI settings. |

Names and types peculiar to the imported example—such as boolean `permalink` or the meaning of `Datei`—are **not product requirements and do not need correction as part of this MVP**.
