# Collections and optional schemas

`Repository::open(root)` discovers JSON collections under `root/data/` when that folder exists, or under `root/` otherwise. A folder with at least one direct regular `*.json` file is a collection. Empty folders are not collections. Its name is the path relative to the data root, using `/` between components; JSON files directly in the data root use the name `@root`. `.editor` and `.git` folders are reserved metadata and are not scanned. Discovery reads directory entries but does not parse record JSON or build indexes.

A schema declaration has a **base folder**. Every JSON file in that folder or any descendant belongs to one collection named for the base. Descendant folders do not become separate collections, even if they contain JSON directly. Schema bases cannot be the same or overlap as ancestors and descendants. Without a schema base, each folder with direct JSON is independent. A malformed JSON file still counts for discovery and can match standalone word or regex searches; it cannot satisfy any field-based filter.

For example, `data/books/a.json` and `data/books/part/a.json` belong to `books` when `data/books` has a schema base. Their IDs are `a` and `part/a`. Without that schema base, `books` and `books/part` are separate collections, both with an `a` record. IDs are relative paths without the `.json` extension; their components cannot be empty, `.` or `..`. Nested file reads refuse symlinked parents and files.

By convention, a schema is read from `<collection>/.editor/schema.json`. The older repository metadata location `.editor/collections/<name>/schema.json` is also supported; a local schema takes priority. A schema can also be supplied explicitly in code, which takes priority over both defaults. A schema-backed collection may have JSON only in descendant folders, but it must have at least one JSON file somewhere in its subtree. An optional `.editor/collections/<name>/ui.json` configures a schema-backed view. Without a schema, a view defaults to its `@id` column and sort; returned parseable records have `no_schema` status. Only schema-backed collections get automatic ordered postings from `"index": true`, validation, and declared reference extraction. The bigram picker and general JSON queries work for every collection. Callers may still build an ordered posting explicitly for a path without a schema.

## Rust API

```rust,no_run
use objekte_core::{Collection, Options, Repository};

// Discover all populated folders; no manifest or schema is required.
let mut repo = Repository::open("my-repo")?;

// Attach a schema to a base relative to my-repo/data/.
let mut repo = Repository::open_with_schema(
    "my-repo", "books", "schemas/books.json",
)?;

// Several bases, an alternate data root, and explicit cache options.
let mut repo = Repository::builder("my-repo")
    .data_root("library")
    .schema("books", "schemas/books.json")
    .schema("people", "/outside/people-schema.json")
    .options(Options::new())
    .open()?;

// Ignore schema files found by convention; explicit schemas can still be added.
let schema_free = Repository::builder("my-repo")
    .without_default_schemas()
    .open()?;

// Build after startup; the same path call replaces an existing posting.
repo.warm_bigram_index("books")?;
repo.build_posting("books", "details.year")?;
repo.build_posting_all("title")?;
repo.rebuild_bigram_index("books")?;

// A collection can also be opened as a standalone metadata primitive.
let plain = Collection::open("my-repo/data/books")?;
let grouped = Collection::open_with_schema(
    "my-repo/data/books", "../../schemas/books.json",
)?;
# let _ = (plain, grouped, schema_free);
# Ok::<(), objekte_core::Error>(())
```

Schema paths passed to `Repository::builder` are relative to the repository root or absolute. Keep schema files outside the JSON data tree or inside a reserved `.editor` folder so they cannot be mistaken for records. `Repository::load_repository(root, options)` continues to load the older `.editor/repository.json` manifest when present, and uses discovery when it is absent. Legacy collection settings may omit `schema` and `ui`; absent `schema` selects a local or shared default when available, then direct-file ownership. `refresh_configuration()` rescans collection boundaries after folders or schema declarations change. `refresh(collection)` updates source files within an existing collection.
