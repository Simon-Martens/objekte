# Structured File Editor

A filesystem backed editor with a reusable Rust project engine and a single Astro server. The Rust engine is available to Node.js through a native N-API module. Astro serves the React interface and JSON endpoints; SQLite stores only users, sessions, invitations, and password resets. Project records and media remain ordinary files.

## Layout

| Path | Responsibility |
| --- | --- |
| `crates/core` | Load one project, parse JSON and Markdown, validate against JSON Schema, discover and search records, resolve references, guard writes, manage media |
| `crates/node` | Native N-API wrapper over the project engine |
| `frontend/src/server` and `frontend/src/pages/api` | Astro server routes, cookie authentication, SQLite user management |
| `frontend/src/components` | React editor with Tailwind and shadcn style components |
| `examples/project` | Sample project with one union schema, data folders, and media |
| `examples/lenz-archiv` | Imported Jakob Lenz Archiv project, including linked Orte and Sammlungen records, and source media |

There is no separate Rust HTTP process.

## Project format

A project has a `.editor/schema.json`, one media directory, and as many data directories as needed. The root schema is Draft 2020-12 with a top-level `oneOf` or `anyOf`. Each top-level variant describes a record type and has `x-collection` metadata. Root `x-project.media` names the media directory. For example:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-project": { "media": "media" },
  "anyOf": [
    {
      "title": "Person",
      "x-collection": {
        "name": "people", "label": "People", "path": "data/people",
        "format": "json", "id": "filename", "display": "name", "search": ["name"]
      },
      "type": "object",
      "required": ["name"],
      "properties": { "name": { "type": "string" } }
    }
  ]
}
```

`format` is `json` or `markdown`. `id` is `filename` or `frontmatter.<field>`. Markdown files map YAML frontmatter and the Markdown body into one object; `body` is reserved for source text. Put shared `$defs` at the schema root so references resolve both in the full union and individual editor forms. The collection is selected by its configured data path, and Rust validates each record against that collection's schema branch as well as the root union. Records do not need a `kind` field. Use `anyOf` when branches may overlap; `oneOf` requires exactly one matching branch.

Each field can use JSON Schema `title` for its friendly name and `description` for help text. The editor shows both beside the input on wide screens. Required fields appear immediately in new records. Record IDs can be entered or generated from the collection's display field in edit mode. Rust checks identifier syntax and uniqueness; generation adds a numeric suffix when needed. Changing an existing ID renames its file and updates a frontmatter ID, if configured. A referenced record must have its incoming references changed first.

`x-editor: relation` plus `x-relation.collection` marks a record ID reference. The engine checks targets when saving and prevents deletion of referenced records. `x-editor: media`, `x-editor: markdown`, `x-editor: inline-markdown`, `x-editor: html`, and `x-editor: json` configure form controls. Markdown and HTML fields use a locally bundled Monaco editor with syntax highlighting and line numbers; preview renders Markdown. The one-line `inline-markdown` editor shows literal text without stored backslash escapes and adds the escapes when the value changes; Raw JSON remains available for writing Markdown syntax directly. Rust rejects line breaks in `inline-markdown` fields. Paths are constrained to the project root, and record writes use revision checks and atomic replacement.

Multiline Markdown editors have an Insert link button and Ctrl/Cmd+K action. The picker searches project media files and records, accepts external URLs, and can embed images. It works in Markdown array items such as Kommentare. The Media view has a references control for each file; Rust scans Markdown links and `x-editor: media` fields in project records and reports the source record and field path. Referenced media files and their parent folders cannot be moved or deleted until those links are changed. Markdown links to records also count as incoming references, so renaming or deleting those records is blocked until the links are changed.

For free text with suggestions, set `x-editor` to `autocomplete`. For a select-style combobox that still accepts new values, use `autocomplete-select`. Both read distinct values from existing records in the same collection through the Rust engine; the in-memory cache refreshes after writes and expires after 60 seconds to pick up external file changes. `x-options` can add fixed suggestions, and `x-autocomplete: false` disables suggestions from records. To narrow suggestions to a parent field, use `"x-autocomplete": { "scope": ["Ort"] }` on the child field. The text control accepts a highlighted suggestion with Tab or Enter; the select control also has a dropdown arrow. Either control allows a new value to be typed and saved.

Collections can declare a category hierarchy in `x-collection.categories`. Each level names a string field in the source record. Add `collection` when the field stores an ID of a separate record; the field must use `x-editor: relation` and `x-relation.collection` for that target. The target collection's `display` field supplies its label, or the category level can set `display`. `itemLabel` names one item in the editor controls. `descriptionField` and `orderField` read metadata from the target record. A child level uses `parentField` to name a relation on the child category record pointing to the preceding level. Rust builds the tree from these records, including empty places and subcollections, and checks that a source record's child belongs to its selected parent. Ordered siblings appear first, followed by unordered siblings sorted by label. For example:

```json
{
  "name": "handschriften",
  "display": "Titel",
  "displayFallback": ["Signatur", "Anmerkung"],
  "categories": [
    { "field": "Ort", "label": "Orte", "itemLabel": "Ort", "collection": "orte", "display": "short", "descriptionField": "description" },
    { "field": "Sammlung", "label": "Sammlungen", "itemLabel": "Sammlung", "collection": "sammlungen", "parentField": "Ort", "descriptionField": "description", "orderField": "order" }
  ]
}
```

The editor changes Ort and Sammlung data by editing their records. Its category views link to those record forms, and linked relation inputs can create records when their schema declares `x-relation.create`. No editor action writes `.editor/schema.json`; edit that file directly and restart Astro to change the schema. For categories without a linked `collection`, optional `categoryEntries` in the schema can still declare static labels and empty nodes. `fieldOrder` sets form order, `displayFields` joins several fields into a record label, and `displayFallback` supplies alternate labels. `showInSidebar: false` hides a collection's list by default; the Sidebar dialog lets each browser show or hide collection lists and category trees. Hidden categories remain navigable in the main collection view.

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

The converter reads `src/dynamic/lists/handschriften/*.html` and writes `data/handschriften`, `data/orte`, and `data/sammlungen`, plus their three schema branches. Subcollection `order` follows the headings in each source file. It does not edit the source checkout or unrelated collections. `--replace` replaces all three target collections, so use it only when you intend to discard edits to those records. Restart Astro after updating the schema or native module.

The current source contains 41 place files and 679 manuscript blocks. Twelve places have 62 group headings; four groups contain context but no manuscript record. Filled fields across the 679 records: Signatur 668, Titel 671, Incipit 107, Umfang 659, Druck 645, Anmerkung 122. Eleven records lack a signature, two signatures repeat within a place, and three blocks are note only. IDs start with normalized place and signature (or title when there is no signature) and end with a short digest to keep them unique. Signatur, Titel, Incipit, and Umfang are converted to one-line Markdown; Druck, Anmerkung, and category context use multiline Markdown. Ort and Sammlung use linked select controls that can create new records; Sammlung choices are scoped to Ort. Links to `/static/` media become authenticated editor media links.

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

Astro exposes `/api/auth/*`, `/api/collections/*`, `/api/relations/*`, `/api/media/*`, and `/api/admin/*` on the same origin as the page. Readers can browse and see incoming references, editors can change records and media, and administrators manage users and invitations. `POST /api/auth/login` sets the session cookie; `GET /api/auth/me` returns the CSRF token. `GET /api/relations/{collection}/{id}/references` lists records that link to an entry. `GET /api/media/search?q=...` searches media files, and `GET /api/media/references?path=...` lists records with Markdown links to a file. Validation failures return HTTP 422 and JSON Pointer issues; stale revisions return 409.

`GET /api/collections/{name}/categories` returns the Rust category tree. `GET /api/collections/{name}/records?category=Ort&category=Sammlung` filters at any declared depth; each repeated `category` parameter is one path level.

The React form covers common JSON Schema controls and offers raw JSON for structures it cannot render clearly. Cookie sessions are implemented; bearer API tokens are outside the current scope.

The native methods run synchronously and searches scan files on demand. For very large projects, indexing or worker threads will be needed to avoid blocking the Astro request loop.
