# Structured File Editor

A filesystem backed editor with a reusable Rust project engine. The new `structured-editor-server` crate serves an Askama/Datastar interface and backend from one Rust process, with Vite-built assets and embedded Turso for administrative data. The existing Astro/React application remains available during migration. Project records and media remain ordinary files.

## Layout

| Path | Responsibility |
| --- | --- |
| `crates/core` | Load one project, parse JSON and Markdown, validate against JSON Schema, discover and search records, resolve references, guard writes, manage media |
| `crates/server` | Axum server, HTML templates, Datastar responses, Turso authentication, and modular field controls; see [its README](crates/server/README.md) |
| `crates/node` | Native N-API wrapper over the project engine |
| `frontend/src/server` and `frontend/src/pages/api` | Astro server routes, cookie authentication, SQLite user management |
| `frontend/src/components` | React editor with Tailwind and shadcn style components |
| `examples/project` | Sample project with independent collection settings, schemas and UI files |
| `examples/lenz-archiv` | Imported Jakob Lenz Archiv project, including linked Orte and Sammlungen records, and source media |

The Rust server implements the [revised MVP](MVP.md): typed drafts and raw JSON, relation/media pickers, related creation, configured trees, suggestions, media management, mapped documents/settings, and account administration. Rust owns validation, persistence and every response that updates the application DOM. Browser code supplies navigation mechanics and source-editor widgets.

## Run the Rust server

From the repository root:

```bash
npm --prefix crates/server/assets ci
npm --prefix crates/server/assets run build
cargo run -p structured-editor-server -- admin-create
EDITOR_INSECURE_COOKIE=1 cargo run -p structured-editor-server
```

Open <http://127.0.0.1:4322>. The default project is `examples/lenz-archiv`; set `EDITOR_ROOT` and `EDITOR_DATA_DIR` to absolute paths to select another project and state directory. The initial administrator is stored in `.data/server.sqlite`, separately from Astro's accounts. See [server setup and architecture](crates/server/README.md) for development mode and extension points. The remaining application-specific development and deployment instructions below describe the existing Astro frontend.

## Project format

Version 2 separates repository configuration from each collection's storage, schema and presentation. There is no persisted union object schema.

| File | Responsibility |
| --- | --- |
| `.editor/repository.json` | Register collection settings files and additional schema-to-file mappings; locate global UI and editor settings |
| `.editor/ui.json` | Sidebar groups, shared presentation defaults and collection placement overrides |
| `.editor/collections/<name>/settings.json` | Data/media type, directory, format, identity, body mapping and permitted operations |
| `.editor/collections/<name>/schema.json` | Standalone Draft 2020-12 schema for a data collection; no UI controls |
| `.editor/collections/<name>/ui.json` | Labels, search, field order, controls, visibility and trees |
| `.editor/settings.schema.json` | Example additional schema, assigned through the same mechanism as any other schema |
| `settings.json` | Settings of this editor instance; not settings for the generated site |

All paths are repository-relative. Filenames and directories above are conventions; registrations explicitly locate each file.

```json
{
  "version": 2,
  "ui": ".editor/ui.json",
  "settings": "settings.json",
  "collections": {
    "people": ".editor/collections/people/settings.json",
    "media": ".editor/collections/media/settings.json"
  },
  "documents": {
    "editor-settings": { "schema": ".editor/settings.schema.json", "files": ["settings.json"] },
    "metadata": { "schema": "schemas/metadata.schema.json", "files": ["config/a.json", "content/b.json"] }
  }
}
```

A data collection's settings file:

```json
{
  "type": "data",
  "path": "data/people",
  "format": "json",
  "id": "filename",
  "schema": ".editor/collections/people/schema.json",
  "ui": ".editor/collections/people/ui.json",
  "operations": ["read", "create", "update", "rename", "delete"]
}
```

`format` is `json` or `markdown`; `id` is `filename` or `frontmatter.<field>`. Optional `idPattern` adds a JSON Schema regular expression to the safe filename/uniqueness checks. Markdown settings use `content: {"body":"body", "frontmatter":"*"}`: the selected property contains the Markdown body and all remaining properties are YAML frontmatter. A different body property is supported. The explicit Record ID operation supplies/synchronizes the frontmatter ID.

A media collection needs `type: "media"`, `path`, `ui`, and operations from `read`, `upload`, `mkdir`, `move`, `delete`. It has no object schema. Multiple media collections have independent directories and permissions. Omitted operations allow all supported operations; an empty array permits none.

Data schemas describe objects, for example `{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}`. Local `$defs` work within each schema; schemas compile offline. Fields are validated against their own collection's schema. UI metadata cannot override `type`, `required` or other validation keywords.

A collection UI file can contain:

```json
{
  "label": "People",
  "sidebarGroup": "Content",
  "position": 0,
  "showInSidebar": true,
  "sidebarAllLinks": "none",
  "display": "name",
  "labels": { "list": ["name"], "search": ["name"], "record": ["name"], "reference": ["name"] },
  "search": ["name"],
  "pageSize": 25,
  "ordering": { "by": "manual", "unordered": "last" },
  "fieldOrder": ["name"],
  "fields": { "/properties/name": { "title": "Name", "x-editor": "text", "x-presentation": {"mode":"singleline"} } }
}
```

Global UI supports `groups: [{"name":"Content","label":"Content","position":0}]`, `defaults: {...}` and `collections: {"people":{"position":1}}`. Collection UI overlays defaults, then global collection overrides apply. All sidebar configuration comes from these files. There are no browser-local navigation preferences.

`sidebarAllLinks` controls the extra **All** links in a collection tree: `"none"` hides them (the default), `"root"` shows only the root link, and `"all"` shows the root and every nested link. The category and container labels remain clickable and open their own views. Parent-owned trees show containers only; individual child records stay in the main lists. `allCollection` can route the root **All** link to a different collection.

### Shared field behavior

`fields` maps JSON Pointers into the collection schema to `title`, `description`, `x-editor`, `x-options`, `x-autocomplete`, `x-create`, `x-visible`, and `x-presentation`. Controls include text, numeric values, boolean, typed enums, Markdown, HTML source, date, date-time, relation, media, arrays and JSON fallback. Arrays support any supported item control, including nested arrays. Objects, unions and tuple arrays use raw JSON. Nullable relations/media accept a JSON string or `null` and also work with pickers.

Check **Include field** to supply a property; absence remains distinct from empty string, zero, false, null and empty arrays. New drafts initialize only explicit schema `default`/`const` values and requested contextual values. Array items without a default start incomplete. Invalid input stays in the draft. Defaults are not injected into existing records.

Ownership is declared in the data schema as `x-ownership: "user" | "generated" | "immutable"`, or `{"kind":"imported","editable":false}`. Omission means user-editable. Hidden values remain in the draft; ownership still applies in raw JSON and core writes. Imported example provenance fields are optional on creation and preserved on update. Arbitrary generated values need a defined system operation or explicit default; the editor invents none.

`x-content: "markdown"`, `"html"`, or `"media"` identifies content semantics. Markdown uses source editing with a sanitized preview. UI `x-editor: "markdown"` and `x-presentation: {"mode":"inline"}` choose compact presentation without escaping or changing source. HTML/templates are shown as source and never executed. Line-break constraints require an explicit schema rule. Dates use `YYYY-MM-DD`; date-times use RFC 3339 with an explicit offset, preserving the entered offset and precision. Text date controls retain invalid input so Rust can report errors without browser coercion.

Enums preserve JSON types and restrict allowed values. Suggestions accept new values: `x-editor: "autocomplete"`, optional `x-options: ["one","two"]`, and `x-autocomplete: false` or `{"scope":["Ort"]}`. Distinct values come from the core cache, refreshed after writes and expired after 60 seconds. Typing queries the full cached set and the server patches only the suggestion list.

### System metadata, references and trees

`_editor` is reserved and added to the effective collection schema by the engine. `_editor.manual_order` is an optional integer scoped to its collection; UI `ordering` controls whether unordered records appear first or last. Ordinary `order`/`Sort` fields have no implicit system meaning.

`_editor.schema_revision` stores the latest schema-changing Git commit whose schema bytes exactly match the schema loaded by the editor. Existing records remain unchanged until saved. An uncommitted/untracked schema produces a visible warning; saving omits any stale schema stamp. This is separate from the content hash used for concurrent-write checks. Schema changes require restarting the server. Automatic schema migration is deferred.

`x-relation: {"collection":"people"}` declares a record reference. The picker searches, resolves configured reference labels, and opens targets. UI `x-create` can define `nameField`, `copyTo`, `parentField`, and `sourceField` for creating a related record without losing the parent draft. Target IDs are validated unless `allowUnresolved` is explicitly set. Incoming references, including Markdown record links, block rename/delete. Markdown link insertion/replacement is a server draft operation using the editor's selected source range.

An explicit media field uses `x-content: "media"` and optionally `x-media: {"collection":"assets"}`; the default collection name is `media`. These fields protect referenced files/folders from move/delete. Markdown media links do not participate in protection. File-existence and type validation remain deferred; pickers browse existing files.

`categories` defines tree levels through `field`, `label`, optional linked `collection`, `display`, `descriptionField`, `orderField`, and `parentField`. Static `categoryEntries` may supply empty nodes. Trees show labels, counts, descriptions, links to their source records, and contextual creation. They impose no validation rules. Labels across list/search/record/reference views share the core's plain-Markdown label behavior.

The manuscript example uses parent-owned child arrays: each Ort contains `sammlungen` and `handschriften`; each Sammlung contains `handschriften`. Arrays contain stable record IDs and declare `x-children: true`, `uniqueItems: true`, and `items.x-relation.collection`. The Rust core validates this acyclic hierarchy and supplies membership, parent lookup, and tree traversal. Record pages combine metadata with independently browsable child lists. A manuscript may appear under an Ort, a Sammlung, or both, while its content is stored once. See [hierarchy configuration and migration](docs/hierarchy.md). The general `x-derived` mechanism remains available for other schemas.

### Editor settings and additional JSON documents

`documents` assigns one schema to one or several explicit JSON files anywhere in the repository. Schemas may describe any JSON value. Files must exist; paths outside the repository, duplicate assignments, and assignments over configuration/schema files are rejected. No glob discovery is implied. Mapped collection records must satisfy both schemas and still use the collection's ownership/reference/revision rules. Assigned files cannot be renamed/deleted until their mapping is changed.

The Rust server exposes **JSON documents** and **Editor settings** screens for opening, validating and saving these files with conflict checks. No concrete editor-setting keys are mandated. Imports preserve editor settings, additional mappings, schemas, assigned file contents and media registrations.

The core still accepts v1 split configuration and legacy `.editor/schema.json`. Both examples and the importers now write v2. Malformed registered configuration never silently falls back to legacy files. No editor action modifies the configuration/schema/UI files. The Astro application remains a legacy UI adapter; the new MVP behavior is implemented in the Rust server.

## Local development

Requires Rust 1.98+, Node.js 26+, and npm.

To recreate the Lenz example from the complete current archive checkout, clone it next to this repository and import it once:

```bash
git clone --depth 1 --single-branch https://github.com/Theodor-Springmann-Stiftung/lenz-archiv.git ../lenz-archiv
cd frontend
npm ci
npm run sample:import
```

The clone includes every current file and only the latest commit's history. The import converts the 22 pages and other list collections to JSON, writes a schema inferred from their frontmatter, and copies `src/static` into the example project's media folder. The six bibliographic and performance lists become Markdown fields: `Eintrag` and optional `Kommentare` for most lists; `Titelzeile` and `Fundstelle` for unselbstständige Drucke; and `Titelblatt`, `Kollation`, `Exemplare`, and optional `Anmerkungen` for selbstständige Drucke. Their source metadata remains in separate fields. Links to `/static/` files become editor media links. The pages retain their HTML templates. Handschriften use a dedicated converter: each manuscript, Ort, and Sammlung becomes a separate linked record; source context becomes record descriptions. It refuses to overwrite an existing import; pass source and destination paths to `npm run sample:import -- /absolute/source /absolute/destination` to build another copy.

To regenerate Handschriften and its linked Orte and Sammlungen collections from the source files, run:

```bash
cd frontend
npm run sample:handschriften -- ../../lenz-archiv ../examples/lenz-archiv --replace
```

The converter reads `src/dynamic/lists/handschriften/*.html` and writes `data/handschriften`, `data/orte`, and `data/sammlungen`, plus their three collection schemas, settings files, and UI definitions. Subcollection `order` follows the headings in each source file. It does not edit the source checkout or unrelated collections. `--replace` replaces all three target collections, so use it only when you intend to discard edits to those records. Restart Astro after updating the schema or native module.

The current source contains 41 place files and 679 manuscript blocks. Twelve places have 62 group headings; four groups contain context but no manuscript record. Filled fields across the 679 records: Signatur 668, Titel 671, Incipit 107, Umfang 659, Druck 645, Anmerkung 122. Eleven records lack a signature, two signatures repeat within a place, and three blocks are note only. IDs start with normalized place and signature (or title when there is no signature) and end with a short digest to keep them unique. Signatur, Titel, Incipit, and Umfang are converted to one-line Markdown; Druck, Anmerkung, and category context use multiline Markdown. Orte own Sammlungen and direct Handschriften lists; Sammlungen own Handschriften lists. Contextual creation attaches the new record to its parent. Existing records can be linked into additional lists without copying their content. Links to `/static/` media become authenticated editor media links.

Then build and run the editor:

```bash
npm run build:native
npm run admin:create
npm run dev
```

Open <http://localhost:4321>. By default Astro loads `../examples/lenz-archiv` and keeps SQLite at `../.data`. Set `EDITOR_ROOT` and `EDITOR_DATA_DIR` to absolute paths for another project. The state directory must be outside the project. `admin:create` prompts for the initial administrator's email address, display name, and password (or accepts them as three stdin lines for automation). Email addresses are the account identifiers and are stored in lowercase.

To clear development login state and generated Astro files, stop the dev server and run:

```bash
cd frontend
npm run dev:reset
npm run admin:create
npm run dev
```

The reset removes `editor.sqlite` and its SQLite sidecars from `EDITOR_DATA_DIR` (or the default `../.data`), plus `frontend/.astro` and `frontend/dist`. It leaves project data and media, installed dependencies, and the Rust build in place. All local users and sessions are deleted, so create a new administrator afterward.

For a production build:

```bash
cd frontend
npm run build:native:release
npm run check
npm run build
npm start
```

`EDITOR_INSECURE_COOKIE=1` permits the cookie over local HTTP. In production the cookie is Secure by default; use HTTPS in front of Astro. The cookie is HttpOnly and SameSite=Strict. Authenticated writes require the CSRF token returned by `/api/auth/me`.

## Containers

```bash
docker compose build
docker compose run --rm editor npm run admin:create
docker compose up
```

The sample Compose file mounts `examples/lenz-archiv`, exposes <http://localhost:4321>, and sets `EDITOR_INSECURE_COOKIE=1` for local HTTP. Import the example before starting Compose. Replace the project bind mount and remove the insecure cookie setting for an HTTPS deployment. The state volume persists SQLite separately from content.

## API

Astro exposes `/api/auth/*`, `/api/collections/*`, `/api/relations/*`, `/api/media/*`, and `/api/admin/*` on the same origin as the page. Readers can browse and see incoming references, editors can change records and media, and administrators manage users and invitations. `POST /api/auth/login` sets the session cookie; `GET /api/auth/me` returns the CSRF token. `GET /api/relations/{collection}/{id}/references` lists records that link to an entry. `GET /api/media/search?q=...` searches media files, and `GET /api/media/references?path=...` lists records with explicit media-reference fields pointing to a file. Validation failures return HTTP 422 and JSON Pointer issues; stale revisions return 409.

`GET /api/collections/{name}/categories` returns the Rust category tree. `GET /api/collections/{name}/records?category=Ort&category=Sammlung` filters at any declared depth; each repeated `category` parameter is one path level.

The React form covers common JSON Schema controls and offers raw JSON for structures it cannot render clearly. Cookie sessions are implemented; bearer API tokens are outside the current scope.

The native methods run synchronously and searches scan files on demand. For very large projects, indexing or worker threads will be needed to avoid blocking the Astro request loop.
