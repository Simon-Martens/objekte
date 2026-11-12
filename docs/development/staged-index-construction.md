# Staged index construction API

`Index::start_staged_build(root, definitions, options)` returns a `StagedBuildTask`.
Its `catalogue()` and `status()` expose discovered file identities while base
construction runs. A file entry is not a record locator until the base has
established its record boundaries. `wait_search_ready()` returns the live
`Index` when complete searches can run. The returned index can keep publishing
optional field capabilities while a query uses a pinned `IndexView` from
`index.view()`. `wait_complete()` additionally waits for the requested
extended work.

`Index::build` remains the blocking convenience path and waits for requested
extended work through the same staged builder. `Index::start_inference` can
later request eligible field discovery on a searchable session without
rebuilding unchanged bigrams. `extended_status`, `wait_extended`,
`pause_extended`, `resume_extended`, and `cancel_extended` control that work.
`quiesce()` stops optional background work before an application exits or
saves a snapshot. A compatible partial snapshot retains published coverage
and pending policy; loading it makes the base searchable before remaining
extended work resumes.

The base pass discovers files, validates and locates records, and builds text
search and indispensable identity/relation metadata. It publishes SearchReady
before the optional pass. The extended pass traverses the published catalogue
in bounded batches and builds configured postings, sorting, explicit
inference, and compact field states together. With no configured selected
fields and no inference request, it performs no second pass. By default the
optional pass uses one build worker so foreground queries retain capacity;
an explicit `RepositoryOptions::build_workers` setting overrides that limit.

Each query captures one data revision and capability revision. A capability
publication changes available acceleration but not record membership or query
meaning. Complete coverage can prove indexed matches; uncovered records are
evaluated from source. An absent posting with incomplete coverage never proves
absence. A failed source read retains the existing source-error contract.
`SearchReady` means correct searches are available, not that every field has
an index or every source read will succeed. Query results and continuations
remain tied to the captured view while later batches publish.

See [field inference](../inference.md), [FILTER grammar](../filter-grammar.md),
and the [integrated measurement report](../ai/generated/staged_index_construction_results.md).
