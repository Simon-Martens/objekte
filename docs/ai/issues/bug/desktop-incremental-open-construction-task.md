# Desktop incremental open does not retain a construction task

- Category: bug
- Originating task: `query-engine-evidence-cache-repair`
- Branch: `integration/query-engine-evidence-cache-repair`
- Base and reproduction revision: `f4b3c9f0dac82315e856c7cd66080d01d779db14`

## Observed behavior

`cargo test --locked --workspace` fails in:

```text
objekte-desktop backend::open::tests::incremental_desktop_open_exposes_catalogue_and_finishes_without_implicit_cache
```

The assertion at `crates/desktop/src/backend/open.test.rs:68` reports:

```text
fresh open must retain construction task
```

The focused desktop test fails identically on the unchanged branch-point
revision, outside any file changed by this query-engine repair.

## Expected behavior

A fresh incremental desktop open should expose a construction task and finish
without requiring an implicit cache.

## Impact and scope

The query-engine core test suite and required workspace check pass. The
workspace test suite remains red because of this pre-existing desktop lifecycle
failure. It was not changed under this task because the task scope is query
matched fields, lifecycle cache exhaustion and line counting.

## Possible next step

Investigate `fresh_open` task publication and the construction-task handoff in
the desktop backend. This is a separate desktop lifecycle repair.
