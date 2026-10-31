# Collections and optional schemas

`Repository::open(root)` reads named schemas from `.objekte/collections/<name>.schema.json`. Each root schema has an `x-objekte` object declaring `name`, repository-relative `path`, `type: "data"`, `format: "json"`, and `id: "filename"`. The name must match its schema filename. The configured path owns all descendant JSON files. The source directory must contain at least one JSON file, and configured paths cannot overlap.

Populated JSON folders without a named schema are still discovered under `root/data/` when that folder exists, or under `root/` otherwise. Each folder with a direct regular `*.json` file is one collection. Empty folders are not collections. Its name is its path relative to the data root, using `/` between components; JSON files directly in the data root use `@root`. `.objekte`, `.editor`, and `.git` folders are excluded from source discovery. Discovery lists directory entries but does not parse record JSON or build indexes.

A schema declaration has a **base folder**. Every JSON file in that folder or any descendant belongs to its named collection. Descendant folders do not become separate collections, even if they contain JSON directly. Without a schema base, each folder with direct JSON is independent. A malformed JSON file still counts for discovery and can match standalone word or regex searches; it cannot satisfy any field-based filter.

For example, `data/books/a.json` and `data/books/part/a.json` belong to `books` when `data/books` has a schema base. Their IDs are `a` and `part/a`. Without that schema base, `books` and `books/part` are separate collections, both with an `a` record. IDs are relative paths without the `.json` extension; their components cannot be empty, `.` or `..`. Nested file reads refuse symlinked parents and files.

For example, `.objekte/collections/books.schema.json` can declare:

```json
{
  "type": "object",
  "properties": { "title": { "type": "string" } },
  "x-objekte": {
    "name": "books", "path": "data/books",
    "type": "data", "format": "json", "id": "filename"
  }
}
```

A schema-backed collection may have JSON only in descendant folders, but it must have at least one JSON file somewhere in its subtree. Only schema-backed collections get automatic ordered postings from `"index": true`, validation, and declared reference extraction. The bigram picker and general JSON queries work for every collection. Callers may still build an ordered posting explicitly for a path without a schema. Views are defined separately under `.objekte/views/`; see [Independent views](views.md).

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

Schema paths passed to `Repository::builder` are relative to the repository root or absolute. Keep them under `.objekte` or elsewhere outside the JSON data tree. `Repository::load_repository(root, options)` uses the new layout when `.objekte` is present; otherwise, it retains support for older `.editor/repository.json` manifests. `refresh_configuration()` rescans collection boundaries and view files after additions or removals. `refresh(collection)` updates source files within an existing collection.
