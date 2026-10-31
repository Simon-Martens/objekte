# Independent views

Views live at `.objekte/views/<name>.view.json`. They are repository-level definitions, not collection settings. A view can query one collection, several collections, or every collection. A future UI can register a view as a collection's `small_list`, `reference_list`, or `autocomplete_list` without changing the collection schema. Core stores no such registration.

Each view has a `name` matching its filename, a presentation `type` (`list` or `single`), optional typed `arguments`, and one or more named `queries`. The type tells the UI how to present the result; it does not limit the cardinality of auxiliary queries. Each query is a native JSON or full-text request, with its own filter/pattern, collection scope, pagination, count, and other query arguments. Named query results are returned together.

```json
{
  "name": "contents_list",
  "type": "list",
  "arguments": {
    "page": { "type": "integer", "default": 1 },
    "from_year": { "type": "integer", "default": 1800 }
  },
  "queries": {
    "contents": {
      "type": "json",
      "request": {
        "filter": "year >= $from_year",
        "parameters": { "from_year": { "$arg": "from_year" } },
        "collections": ["contents"],
        "limit": 25,
        "offset": { "$page_offset": { "page": { "$arg": "page" }, "size": 25 } },
        "count": true
      }
    }
  }
}
```

An argument can be `string`, `integer`, `number`, `boolean`, `array`, or `object`. A missing `default` makes it required. `{"$arg":"name"}` substitutes its typed JSON value anywhere in a query request. `{"$page_offset":{"page":...,"size":...}}` calculates a zero-based offset from a one-based page number and a positive page size. The usual query request fields remain available without a view argument. A caller may override any native field for one named query; unknown fields are rejected by that query API.

Run a view in Rust with `Repository::run_view(name, &arguments)` or `run_view_with_query_arguments(name, &arguments, &per_query_overrides)`. The CLI uses the same path:

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents_list --arg page=2
cargo run --release -p objekte-cli -- examples/musenalm view contents_list --arg from_year=1820 --query-arg contents.limit=10
```

View definitions are checked when the repository opens. Existing view files reload when their bytes change; `refresh_configuration()` discovers additions and removals. The current TUI remains a general search interface and does not register default views.

For older `.editor` repositories, the former collection `view` command and its list settings remain available as a compatibility path. New `.objekte` repositories use only named views.
