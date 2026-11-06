# Construction file-entry clone comparison

A is unmodified `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`; B is the
repair worktree with the file-entry clone removed. The B binary was compiled
before committing; the accompanying commit identifies the patch. Other
intervening repairs affect queries, not construction. The six independent
ASCII snapshot-disabled builds ran A-B-B-A-A-B on the same physical corpus,
release mode, Rust 1.99.1, affinity `0-31`, `powersave` governor, and OS-warm
files. Commands and raw outputs are in `manifest.json` and
`measurements.jsonl`.

| Variant | Wall times s | Median wall s | Median peak RSS MiB | Parses |
|---|---|---:|---:|---:|
| A | 11.722, 11.696, 11.763 | 11.722 | 236.3 | 148,486 |
| B | 11.624, 11.742, 11.708 | 11.708 | 236.2 | 148,486 |

The 0.014 s median difference is below run variation, so no measurable build
speedup is claimed. The deterministic reduction is one full `FileEntry` clone
per selected file. The build now copies only its `SourceRevision` and borrows
the live inventory entry after any diagnostic mutation. Healthy files retain
empty diagnostic vectors, and mixed JSONL diagnostic tests continue to pass.
The existing estimated index size excludes inventory allocations; final build
measurements will keep resident and peak process memory separate.
