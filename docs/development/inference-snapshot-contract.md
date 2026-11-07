# Inference and snapshot performance contract

Implementation base: `4fd47bcccd1affab1f98fff1db5cff21e6b73ce5` on 7 November 2026.

Repository opening has three load policies: `Disabled` builds from source,
`IfValid` restores a current compatible snapshot or builds from source, and
`Required` restores or returns an error. `Repository::open()` defaults to
`IfValid`. None of these modes writes a snapshot. A caller persists a built
repository with `Repository::save_snapshot(path)`; that operation retains
atomic replacement and `sync_all` durability. Snapshot content compatibility
does not depend on the load policy.

The CLI maps no flags to `Disabled`, `--load-index` to `Required`,
`--write-index` to `Disabled` followed by one explicit save, and both flags
to `IfValid` followed by a save only when a fresh build was needed. The CLI
TUI uses the same rule during its repository loading step.

An unsaved rebuild may be repeated by the next opening. Opening leaves stale,
corrupt, and incompatible snapshot files untouched. Build, restore, and save
timings have separate boundaries.

The starting inference policy and field output must remain stable while
reducing traversal and dictionary allocation. Timing comparisons use matching
corpus, schema, inference mode, alphabet, workers, and release toolchain.
