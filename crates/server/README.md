# Rust editor server

`structured-editor-server` serves both HTML and backend commands through Axum. MiniJinja renders the persistent admin shell and replaceable workspace fragments. Datastar applies the backend's HTML patches. Vite builds CSS, the pinned Datastar runtime, and lazily loaded Monaco controls. No Node process is needed in production.

This implements the revised repository MVP. The Astro application remains a separate legacy frontend on its existing port.

## Run

Requires the repository's Rust toolchain and Node/npm for the asset build. From the repository root:

```bash
npm --prefix crates/server/assets ci
npm --prefix crates/server/assets run build
cargo run -p structured-editor-server -- admin-create
EDITOR_INSECURE_COOKIE=1 cargo run -p structured-editor-server
```

Open http://127.0.0.1:4322. `admin-create` prompts for email, display name, and a password of at least 12 characters. Noninteractive use accepts those as three stdin lines. It only creates the first administrator.

To delete all Rust server accounts and sessions, stop the server and run `cargo run -p structured-editor-server -- reset`, then run `admin-create` again. `reset` removes `server.sqlite` and its SQLite sidecars from `EDITOR_DATA_DIR` (default `.data`). It leaves project records and media, built assets, and the Astro frontend's separate `editor.sqlite` intact. The command succeeds when the database does not exist.

| Variable | Default / meaning |
| --- | --- |
| `EDITOR_ROOT` | Workspace `examples/lenz-archiv`; repository to edit |
| `EDITOR_DATA_DIR` | Workspace `.data`; must be outside the repository |
| `EDITOR_BIND` | `127.0.0.1:4322` |
| `EDITOR_ASSETS_DIR` | This crate's `assets/dist` |
| `EDITOR_DEV` | Set to `1` (or pass `--dev`) for live Vite assets, disk templates and HTTP cookies |
| `EDITOR_VITE_URL` | `http://127.0.0.1:5173` in dev mode; otherwise unset. Override with a local Vite HTTP origin |
| `EDITOR_INSECURE_COOKIE` | Optional `1` for HTTP outside dev mode; dev mode automatically omits `Secure`. Otherwise cookies require HTTPS |
| `RUST_LOG` | `structured_editor_server=info` |

Use absolute filesystem paths for deployment. Copy the release binary and built `assets/dist`; set the paths above. Terminate HTTPS at the reverse proxy. Repository configuration is loaded at startup, so restart after changing it. The existing Docker/Compose setup still targets Astro.

## Development with live reload

Install the assets once with `npm --prefix crates/server/assets ci`, then use two terminals from the repository root:

```bash
# Terminal 1 — CSS/TypeScript updates and HTML template watching
npm --prefix crates/server/assets run dev
```

```bash
# Terminal 2 — keep the Rust server running while editing the UI
cargo run -p structured-editor-server -- --dev
```

Open **http://127.0.0.1:4322**, the Rust server URL. Run `cargo run -p structured-editor-server -- admin-create` once if you have no administrator yet.

`--dev` (equivalently `EDITOR_DEV=1`) enables:

- Vite at `http://127.0.0.1:5173`, without needing an `assets/dist` build. CSS updates in place; TypeScript updates use Vite's reload behavior.
- HTML templates loaded from `crates/server/templates/` on each render, including nested includes. Vite watches this folder and refreshes the page when an HTML file changes. Rust stays running, preserving server-side edit sessions.
- Session cookies without `Secure` for local HTTP. `HttpOnly`, `SameSite=Strict`, authentication and CSRF protection still apply.

To use another Vite port, set the same `EDITOR_VITE_URL` in both terminals and pass that port to Vite, for example `EDITOR_VITE_URL=http://127.0.0.1:5174 npm --prefix crates/server/assets run dev -- --port 5174` and `EDITOR_VITE_URL=http://127.0.0.1:5174 cargo run -p structured-editor-server -- --dev`.

Rust source changes (including HTML built by Rust functions) still require a rebuild/restart. Repository configuration changes require a restart. HTML template and asset changes do not. A full browser refresh reloads the last submitted draft; submit input you want to keep before changing a template.

Without dev mode, templates are embedded in the binary and parsed at startup, assets use the built Vite manifest, and cookies require HTTPS by default. There is no runtime dependency on the source template folder or Node. Recompile after production template changes and restart after a production asset build.

## Read caching and concurrent editing

The core caches parsed collection records, prepared list values, paginated query results, ID-to-file lookups and incoming-reference indexes. Reads check file metadata for external changes; on Unix this includes modification time, change time, size and file identity. Additions, deletions and atomic replacements invalidate affected results without a TTL. List and reference dependencies include other collections so changed relation labels and derived values are refreshed. Reference indexes also track mapped documents and settings. Parsed-record entries are capped at 10,000; prepared lists and sorted queries each retain up to 16 entries.

Metadata directory scans still run to detect external edits. New searches filter cached values in memory; pagination reuses sorted results. Filename-based child lists load only their members. Configuration and schema changes still require restarting the server.

Editing commands serialize per draft, rather than holding the global session registry during file access, validation or rendering. Related-record saves lock parent and child drafts in a consistent order. Version checks continue to reject stale commands. The project write lock remains in place to protect reference integrity across writes, and blocking work stays on Tokio's blocking pool.

## Boundaries

| Location | Responsibility |
| --- | --- |
| `crates/core/src/draft.rs` | Typed draft decoding, absent versus present values, invalid input preservation, stable array item identities and structural operations; no HTTP or HTML |
| Other core modules | Schema loading, validation, ID generation, search, references, file persistence and revision checks |
| `src/sessions.rs` | Bounded per-login editing sessions and monotonically increasing request versions |
| `src/fields/` | Web control registry and small control-specific renderers |
| `templates/fields/` | Control markup |
| `templates/shell.html` | Admin frame with stable navigation, workspace, notification and overlay regions |
| `src/templates.rs` | Shared HTML escaping, embedded production templates and disk loading in dev |
| `src/views.rs` and remaining templates | Collection lists, previews, edit screens and HTML patch serialization |
| `src/routes.rs`, `media.rs`, `documents.rs`, `accounts.rs`, `pickers.rs` | HTTP adapters, commands and full-page/fragment responses |
| `src/auth.rs`, `migrations/` | Embedded Turso users, login sessions, invitations and password resets |
| `assets/src/controls/` | Optional browser widgets; Monaco owns only its editor-internal DOM |
| `assets/src/main.ts` | History and unsent-input navigation warning; no record mutation or list reconciliation |

To add a control, implement `Control` in a new module under `src/fields`, add its template, and register it before the scalar fallback. Add a browser component only if normal HTML inputs are insufficient. Keep data semantics and reusable draft operations in the core crate. Existing core validation always runs on save; browser controls never authorize a write.

The server reads version 2 per-collection configuration through `Editor`, with v1/legacy loading retained by the core. See the root README for the configuration contract. The admin shell is independent of concrete collections.

## Interface and browser components

The administration shell uses self-hosted Source Sans 3, JetBrains Mono and Remix Icons. CSS theme tokens in `assets/src/style.css` define light and dark palettes. The header selector persists the choice in local storage; the system color preference supplies the initial default. Monaco switches themes with the shell. Desktop lists use compact columns, while narrow screens use stacked records and a collapsible sidebar.

Tippy.js (with Popper) positions tooltips for hover and keyboard focus, including controls added by server patches. Tooltips follow the active theme and reduced-motion preference, support Escape dismissal, and keep labels as text. Array editors use compact item controls without an extra enclosing card; record previews render each array item using its field type, including Markdown, references and nested arrays. Optional fields are available in the Add field menu.

`code-input` lazily loads Monaco 0.57 for Markdown and HTML, including locally bundled editor/HTML workers. The server-rendered textarea remains the form transport and a usable fallback. Only Monaco's internal host is excluded from Datastar morphing. Server version changes synchronize source without rewriting unchanged models; removed controls dispose their models, subscriptions and resize observers. Compact Markdown fields still preserve multiline source. Ctrl/Cmd+K requests the server's Markdown link picker; Ctrl/Cmd+S submits the active form. Tab moves between fields by default (Ctrl+M toggles Monaco's tab behavior).

Live validation is debounced (400 ms by default, configurable in instance settings) and runs against a draft snapshot in Rust. Responses patch feedback regions only, preserving typing, selection and undo history. Validation never advances the stored draft or saves a file; superseded requests are cancelled, and stale draft versions return no feedback. Settings and schema-assigned JSON documents use the same feedback-only approach. Explicit save still performs authoritative validation and revision checks.

Browser components under `assets/src/controls/` keep editor lifecycle, theme choice, picker focus and validation scheduling separate. The server owns field values, picker choices, list content and every CRUD patch. Media reference disclosures request their fragments on demand instead of scanning the repository for every file in a list.

Record actions open a deletion review showing parent list placements and other incoming references. Editable placements can be removed directly in the review with a parent revision check. This saves the list immediately; deleting the record is a separate, explicit action. Other references still block deletion and link to their source records. The core rechecks all references and the record revision on the final delete.

Request errors are sent as notification patches for Datastar requests, with HTTP 200 for delivery and the original status in `X-Editor-Error-Status`. Normal HTTP requests retain the original error status. This matters because Datastar does not process non-200 response bodies. Connection failures show a shell notification. Form commands explicitly set both `retry:'never'` and `retryMaxCount:0`, so a lost connection cannot silently resubmit a write later.

## URL and command contract

| Request | Result |
| --- | --- |
| `GET /collections/{collection}?q=…&offset=…` | Searchable, paginated list; `tree` contains a JSON array of tree values |
| `GET /collections/{collection}/records/{id}` | Record preview |
| `GET /collections/{collection}/records/{id}/delete` | Review deletion, list placements and incoming references |
| `POST /collections/{collection}/records/{id}/delete/unlink` | Remove one placement with the parent revision; return to deletion review |
| `POST /collections/{collection}/edit-sessions` | Start a new-record draft |
| `POST /collections/{collection}/records/{id}/edit-sessions` | Start an existing-record draft |
| `GET /collections/{name}/records/{id}/edit`, `/collections/{name}/new`, `/documents/{name}/edit?file=…` | Stable editor entry URLs; start/resume a private tab draft |
| `GET /edit-sessions/{session}` | Render the draft owned by this login |
| `POST /edit-sessions/{session}/save` | Validate, create/update/rename, render canonical saved data |
| `POST /edit-sessions/{session}/live-validation` | Validate submitted input on a clone and patch feedback only |
| `POST /edit-sessions/{session}/validate` | Validate without writing |
| `POST /edit-sessions/{session}/generate-id` | Generate an available ID through the core |
| `POST /edit-sessions/{session}/raw`, `/form`, `/check-id` | Whole-record JSON, structured controls, ID checks |
| `POST /edit-sessions/{session}/fields/{field}/suggestions` | Search cached values and patch only the datalist, without changing the draft |
| `POST /edit-sessions/{session}/fields/{field}/actions/{action}?item={item}` | Array operations, reference/media pickers, related creation, Markdown preview/link insertion |
| `GET /media/{collection}?path=…&q=…&offset=…` | Browse or search a media collection |
| `GET /media/{collection}/references?path=…` | Load explicit incoming file references on demand |
| `GET /media/{collection}/file?path=…` | Authenticated file access |
| `POST /media/{collection}/upload`, `/actions/{mkdir,move,delete}` | Media commands |
| `GET /documents`, `GET/POST /documents/{mapping}?file=…`, `GET/POST /settings` | Assigned JSON documents and editor settings |
| `POST /settings/validation`, `/documents/{mapping}/validation` | Validate JSON and patch feedback without saving |
| `GET/POST /account`, `POST /account/password` | Profile and password management |
| `GET /admin`, `POST /admin/{action}` | Users, roles, disable/enable, revoke sessions, invitations and reset links |
| `GET/POST /auth/invitations/{token}`, `GET/POST /auth/resets/{token}` | Redeem one-time tokens |
| `POST /collections/{collection}/records/{id}/delete` | Revision-checked deletion with incoming-reference protection |

Ordinary page requests return the full shell. `Datastar-Request: true` requests return short-lived SSE batches containing `datastar-patch-elements` events. Commands patch the workspace and any affected navigation/notification regions together. The browser neither fabricates records nor decides which list rows to replace. POST is also supported by ordinary HTML forms. There is no duplicate JSON CRUD API.

Every mutation requires a login and CSRF token; content changes require editor/administrator permissions. Draft commands additionally require the current draft version. Saving and deleting use the core's on-disk revision checks. Invalid raw input remains in the draft; no save happens until validation passes. A stale draft request receives HTTP 409 without applying its submitted input. A file conflict keeps the draft for manual review; reopen the record to obtain a new revision.

Drafts live in bounded process memory (32 per login, 512 total). A background sweep runs once a minute and removes drafts after two hours without a draft request; logout also removes that login's drafts. Restarting the server loses drafts. Record edit URLs are `/collections/{name}/records/{id}/edit`, new records use `/collections/{name}/new`, and documents use `/documents/{name}/edit?file=…`; `node` selects a subtree. The private draft ID stays in the form and tab-scoped storage. Refresh resumes the same draft and restores pending form input from tab storage when the version still matches. Other tabs use independent drafts; a BroadcastChannel claim prevents active tabs from sharing a copied draft ID. Browser storage is optional; when unavailable, automatic refresh recovery is unavailable.

Save keeps the editor open. The toolbar offers Discard without a separate Close action. Discard releases the draft and returns to the saved entry view; a new unsaved record returns to its parent or collection. Expired or missing drafts show an explicit explanation and Reopen editor action. Navigation away leaves the draft available until expiry, while the internal close command explicitly removes it. Drafts never hold record locks between requests.

## Storage and current coverage

Turso is used as the embedded SQLite-compatible engine through the Rust `turso` crate. `server.sqlite` stores users and hashed login tokens, with Argon2 password hashes and versioned migrations. This is local embedded storage; it does not connect to Turso Cloud or sync databases. The database is separate from Astro's `editor.sqlite`; existing accounts are not imported. Project content continues to use the core's filesystem storage.

Implemented controls: explicit single/multiline text, integer/number, boolean, typed enum, date/date-time, Markdown/HTML source, reference/media pickers, arrays of all supported types (including nested arrays), system metadata and JSON fallback. Explicit presence distinguishes omission from empty strings, false, zero, null and empty arrays. Hidden/imported/immutable values are preserved according to ownership. New records use explicit defaults/context only. Frontmatter IDs are supplied by the Record ID control and synchronized by the core.

Implemented workflows include configured sidebar groups and trees, contextual creation, view-specific labels, autocomplete, incoming-reference views, whole-record JSON, source-range Markdown link insertion, media collections, additional schema-assigned JSON documents and editor settings. Media upload is limited to 50 MiB per file; uploads never overwrite existing files. Directories can move with reference protection; only empty directories can be deleted. Downloaded active content is sent as an attachment with a sandbox policy.

Account administration includes profile edits, current-password verification, last-administrator protection, account disable/enable, session revocation, expiring single-use invitations and resets. Administrators obtain one-time links to share manually; there is no outbound email service. Password changes/resets and role/status changes revoke sessions. Tokens are hashed in Turso.

The MVP intentionally defers arbitrary panel/screen composition, conditional/table controls, automatic schema migration, and media existence/type validation. Nested objects use recursive property forms and document-tree navigation. Embedded Turso is the selected local engine, not a Turso Cloud integration.

Normal text fields contain decoded text: users enter quotes, newlines and backslashes as ordinary characters. JSON/YAML quoting happens automatically at save time. Markdown source controls omit redundant imported punctuation escapes only when the rendered content remains identical; meaningful escapes, code and literal backslashes are preserved. Lists and search excerpts render Markdown as readable text, including array items.

Markdown preview is sanitized. HTML fields are shown as source, never executed as project templates. Datastar 1.0.4 is vendored from its official bundle with its MIT license, avoiding the deprecated npm beta package. The asset lockfile pins Vite and Monaco dependencies.

## Verification

```bash
cargo test --workspace
npm --prefix crates/server/assets run build
```

Server integration tests cover authentication/roles/CSRF, invitation and reset replay, session revocation, full-page/fragment responses, invalid/stale drafts, file conflicts, arrays, generated IDs, related creation, suggestion-only patches, media multipart upload, feedback-only live validation, mapped documents, raw editing and Markdown insertion. Core tests cover independent configuration, ownership, derived relationships, manual order, exact committed revisions, date formats, explicit media references and draft preservation. Browser verification should use a temporary copy of a project because saves operate on real files.

## Configuring collection lists and editing

Each collection's `ui.json` can define:

```json
{
  "icon": "ri-quill-pen-line",
  "list": {
    "columns": ["Titel", "Signatur", "Ort"],
    "defaultSort": { "field": "Titel", "direction": "asc" },
    "filters": ["Ort", "Sammlung", "Verschollen"],
    "autoFilters": false
  },
  "edit": { "sidebarFields": ["Verschollen"] }
}
```

Columns use top-level schema field names, plus `_label` and `_id`. `_manual` is supported as a default sort key. Column checkboxes update the list immediately and keep the chooser open; Done dismisses it. Runtime column choices, sort direction, search, category and filters are URL parameters and survive pagination. Sorting does not rewrite stored order. Numeric columns sort numerically; missing values remain last. Search returns matching field values, including fields outside the selected columns, with escaped Unicode-aware highlighting.

`filters` chooses desirable fields explicitly. `autoFilters: true` also derives filters from visible scalar fields: numbers get ranges; booleans get true/false/not-set; relations, enums and strings with at most 40 distinct values get selectors; larger string vocabularies get a contains input. Rich text and hidden fields are not automatically included. Counts are computed over the category before query/filter application, with a separate matching count. The Rust core owns query semantics, matching, counts and ordering.

Absent fields are not rendered as empty controls. Add field inserts them into the draft; the trash action removes them. The bounded draft history keeps up to 100 changes, including invalid input, presence and array changes. Undo/redo buttons submit server commands. Ctrl/Cmd+Z and Ctrl/Cmd+Shift+Z (also Ctrl+Y) use native text/Monaco history while typing, then server history at a structural boundary. Undo changes the draft, not files already saved on disk. History restoration replaces the editor fragment so dirty checkbox, select and text-control properties also match the restored values; ordinary edits and live validation retain morphing. Preview toggles are independent per Markdown field and update through the debounced validation response. Monaco always shows line numbers.

## Instance UI defaults

`settings.json` is still editor instance data. The example's `.editor/settings.schema.json` now declares `defaultTheme`, `density`, `pageSize`, `editorFontSize`, `validationDelay`, `markdownPreview` and `mediaThumbnails`. `/settings` generates controls from that schema and uses normal validation and revision checks. Unrelated existing properties are preserved. A user's saved color choice overrides the instance default. The example settings values remain empty until explicitly saved; schema defaults supply the initial controls.

## Media and invitations

Media operations now include separately configurable `replace` and `copy` capabilities. Replace keeps the target path, requires the current content revision, rejects a different filename extension and writes atomically. Existing references therefore keep their target. Duplicate creates a new path and refuses overwrite. Upload and replacement retain the 50 MiB limit. Lists provide image thumbnails, download and copy-path controls. Move/delete continue to protect explicit incoming references; links embedded in Markdown are not currently managed file references.

Invitations and reset links include locally generated QR codes and a copy control. Set `EDITOR_PUBLIC_URL` to the editor's HTTP(S) origin behind a proxy so links and QR codes use the externally reachable address. Otherwise the request Origin, or HTTP Host for ordinary form submissions, supplies the base. A phone cannot reach a server bound only to localhost. Tokens remain one-time, expiring, and are shown only to an administrator; no external QR service receives them.

See [the archive field review](../../docs/lenz-archiv-field-review.md) for evidence-based proposals for historical dates, attachment lists, contributor relations and other future types.

## Inherited view options

UI option objects merge recursively: omitted options inherit, local values override, and arrays (such as `columns`) replace the inherited array. Set `inherit: false` on an option object to discard its inherited options; omitted options then use the renderer's built-in defaults. The reset is local and does not modify other views. Presentation never changes schema validation or stored data.

Collection defaults resolve from global `defaults`, the collection UI file, then global `collections.<name>` overrides. Child lists start with their target collection's resolved UI, followed by the parent collection's `childLists.defaults`, then `childLists.fields.<property>`. The shared defaults apply to sibling child lists on that parent; each property can override them independently. For example, in an Ort's collection UI:

```json
{
  "childLists": {
    "defaults": { "list": { "previewSize": 8 } },
    "fields": {
      "handschriften": {
        "list": {
          "columns": ["Signatur", "Titel"],
          "defaultSort": { "field": "Signatur", "direction": "asc" }
        }
      },
      "sammlungen": {
        "inherit": false,
        "list": { "columns": ["_label"], "previewSize": 4 },
        "pageSize": 25
      }
    }
  }
}
```

Embedded previews and their **Browse list** pages use the same resolved columns, sort, and filter configuration. `list.previewSize` controls preview rows (default 10, range 1–200); `pageSize` controls the full list. Preview columns are no longer capped at four. These overrides belong to that parent/list occurrence and do not alter the target collection elsewhere. They do not propagate across reference navigation into another record's child lists.

Each child table in an object's view also has a **Columns** selector. Checkbox changes update that table immediately and keep its selector open. Each table's choices are stored separately in the page URL, survive reloads, and carry into **Browse list**. They do not change the configured defaults or saved records.

General assigned JSON documents now offer a typed preview and an **Edit** button in the document list, using the same draft controls, array actions, undo/redo, validation, and conflict-checked saves as records. Documents keep their assigned paths and have no record-ID controls. Arrays and scalar values are also supported at the document root. The dedicated `/settings` screen remains available.

Document UI options live in global `documents.<mapping-name>` and inherit global `defaults`. `fields` uses the same schema JSON pointers as collection UI. `presentation` supplies defaults down the schema tree; a field's `x-presentation` overrides or resets them. For example:

```json
{
  "defaults": { "presentation": { "editor": "form" } },
  "documents": {
    "metadata": {
      "fieldOrder": ["title", "source"],
      "fields": {
        "/properties/source": { "x-presentation": { "editor": "raw" } },
        "/properties/tags/items": {
          "x-presentation": { "inherit": false, "editor": "form" }
        }
      }
    }
  }
}
```

`presentation.editor: "raw"` at the collection/document root opens raw JSON editing by default. `x-presentation.editor: "raw"` on a field renders that entire value/subtree as JSON. `"form"` selects normal typed controls; the existing `x-editor: "json"` remains an explicit JSON control. Raw editing preserves JSON types and still validates against the same schema. A raw subtree is one editor, so its descendants have no separate visible controls until that subtree is shown as a form. Objects and homogeneous object arrays use recursive subtree views. Scalar arrays stay inline. Unions, mixed arrays, tuple schemas, invalid container values and unsupported schema shapes use the validated JSON fallback when enabled. Reload the server after changing configuration.

The header button beside **objekte** shows or hides the sidebar. Drag its right divider to resize it, or focus the divider and use Left/Right (Shift for larger steps), Home, or End. Sidebar width and desktop visibility are remembered locally; navigation content and ordering still come from project UI configuration. List links open previews; existing records and assigned documents offer Edit buttons both in their list rows and at the top right of their individual views, subject to editing permissions.


### Recursive subtree navigation

Objects and homogeneous object arrays appear in the document tree for records and assigned JSON documents. Opening a branch shows its properties or items; edit navigation submits visible inputs into the existing draft before changing branches. Save validates and saves the entire document. Absent schema-defined objects can be added one level at a time, including recursive local `$ref` schemas. Empty scalar arrays remain inline; adding unknown properties or choosing a type for an untyped empty array still requires an explicit raw editor or a schema.

Inherited presentation options (under `presentation` in collection/document UI, or `x-presentation` on a schema field):

```json
{ "x-presentation": { "navigation": "stop", "rawFallback": false } }
```

`navigation: "stop"` makes this object an endpoint. Its scalars and homogeneous scalar arrays remain structured. Deeper containers use fallback unless a local `navigation: "continue"` overrides the stop. `rawFallback: false` leaves unsupported values preserved and unavailable for structured editing. `editor: "raw"` explicitly enables a JSON editor for a subtree even when automatic fallback is disabled. `inherit: false` clears inherited presentation options. The explicit whole-document Raw JSON action remains available.

The renderer has a safety boundary after 64 nesting levels; deeper values use the same fallback policy and remain preserved. Navigation does not change JSON storage, reference semantics, schema validation, or ownership rules.


Removal actions describe the stored data: **Unlink from this list** removes an ID reference while retaining the separate record; **Delete item** removes an embedded array value and its nested contents on save; **Remove field** leaves a property unset on save. Child-list unlinking updates the saved parent immediately; reference-array unlinking happens in the draft. Linked records also offer **Delete <type>…** (or **Delete record…** without a configured `singularLabel`), opening the existing incoming-reference review. Unlink and delete permissions are checked independently. Deletion remains blocked while saved references exist; opening its review does not save an editing draft.


ID references declared with `x-relation` are checked by the server against their target collection before saving, including nested arrays/objects, mapped JSON documents, and raw editing. Missing or invalid IDs produce errors at the JSON path and leave the saved file unchanged. `allowUnresolved` no longer bypasses validation. Incoming references from mapped documents/settings participate in deletion checks and automatic rename updates. Standalone document saves recheck references under the same write lock as deletion. External edits to repository files can still introduce invalid data; the editor rejects saving it until the references are repaired.

Changing **Record ID** and saving automatically updates incoming relation fields (including nested values and child lists), self-references, mapped JSON documents, settings, and Markdown record links. **Save** and **Save & Close** report the number of reference occurrences renamed. Plain text and references to other collections are preserved. Markdown link labels, titles, and surrounding source formatting are preserved, including reference-style links. The operation validates all changed files against the final IDs, stages file contents before writing, and rolls back completed writes on ordinary errors. Existing revisions and target-ID conflicts are checked; a stale draft cannot overwrite an automatically updated reference. Referencing collections must permit updates, and protected fields and schema constraints still apply. Like child creation, this multi-file operation is not crash-atomic.
