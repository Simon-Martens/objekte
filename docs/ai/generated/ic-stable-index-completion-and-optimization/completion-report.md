# Stable-index completion and optimization report

Task: `ic-stable-index-completion-and-optimization`  
Date: 16 November 2026  
Reviewed repair head: `1022768be3d625be7acec28da38defd831b85fe1`  
Measured implementation: `da70c3995a6f08989a7d51f22f33ddc868643c0e`  
Native baseline: `c54486f893a57fac0b1b80ed967ab4f5a5fd26c9`  
Branch: `integration/ic-stable-index-completion-and-optimization`  
Worktree: `/home/simon/source/objekte-integration-ic-stable-index-completion-and-optimization`

## Classification

**Incomplete implementation with required evidence and ownership work remaining.**
The lifecycle, composite mutation, failure-boundary and measurement gaps were
closed, and O01–O03 were evaluated. R02 (owned component conversion), R03
(all-family independent owner merging/bounded waves) and matched native R19
distributions remain incomplete. No production or default backend change was
made.

## Implemented in this phase

- Production `prepare_file_change` parses exactly one affected physical file
  through the existing adapter/batch path, producing records and complete typed,
  length, state, gram, sort/composite and relation facts without rebuilding a
  repository.
- `lifecycle` benchmark now applies real prepared facts, changes source content
  in one replacement, keeps it unchanged in another, checks stable typed results
  against a fresh native rebuild, saves/reopens and reports preparation,
  application, storage growth and snapshot size separately.
- Composite keys rebase checked local slots during prepared mutation. Regression
  covers a changed composite at a nonzero slot base and after reopen.
- Prepared-change validation covers duplicate collections, membership,
  component/collection/key-role consistency, local slot bounds, relation
  metadata decoding, source existence and destination collisions.
- Application-started tracking blocks queries/saves after late append, delete,
  move and posting failures. Fault tests cover append/delete/move after the
  first write; invalid preparation leaves the owner usable.
- Campaign driver aggregates medians/ranges across corresponding samples rather
  than taking stage/RSS values from the last run, and identifies one selected
  trace separately.
- O01 sorted insertion, O02 catalog/path cleanup, and O03 capacity control were
  measured and rejected; O04 was not run because no profile justified it.

## Final matched measurements

ASCII P=8, five alternating fresh-process pairs:

| Measurement | Native | Current repair | Candidate |
| --- | ---: | ---: | ---: |
| Median construction | 1.595 s | 3.027 s | 3.031 s |
| Ratio vs native | 1.00x | 1.90x | **1.90x** |
| Resident RSS | 400.5 MB | — | 772.4 MB (1.93x native) |
| Peak RSS | 413.7 MB | — | 841.2 MB (2.03x native) |
| Stable payload | — | — | 407.5 MB |

Unicode P=8, three pairs: native 1.751 s, current repair 3.393 s, candidate
3.261 s; **1.86x native**, 3.9% better than the current repair. Resident/peak
RSS were 1.85x/1.95x native.

Synthetic P=8: native 0.735 s, candidate 1.502 s; **2.04x native**, 141
field-job overlap pairs.

Full Musenalm validation passed with zero semantic differences.

## Persistence, primitive and lifecycle evidence

- Snapshot save: 223 ms for 1,323 components / ~407 MB.
- Manifest open: 29.4 ms; first 27.9 MB component load: 24.8 ms.
- Primitive point medians: 1.68-2.16 µs; p95 1.75-2.26 µs over 200 repetitions
  after component/live-domain preload.
- Real prepared lifecycle: 10 replacements, preparation 6.5 ms total,
  application 50.9 ms total, 20 changed-age matches against native, save 2.1 ms,
  snapshot 2.62 MB. Dead slots 2,000; reopened stable payload 2.42 MB.
- Standalone atomic save/open, generation lazy loading and fault tests remain
  green.

## Required checks

```text
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype
cargo test -p objekte-search-core --features ic-stable-prototype
cargo check -p objekte-search-core --all-targets --features ic-stable-prototype,detailed-profiling
cargo test --workspace --features objekte-search-core/ic-stable-prototype
```

Workspace check/tests and all feature/profiled combinations passed on this
branch. 49 focused stable-index tests pass. The previously observed desktop
non-UTF-8 test failure also reproduces on the untouched native baseline in this
environment.

## Unmet mandatory work

1. R02: primary source conversion still builds `CollectionState` and passes a
   borrowed collection graph into `from_collections`. Native posting/text/sort
   maps are not represented by owned `ComponentBuildJob` inputs.
2. R03: posting fields merge in parallel, but custom bindings, packed bigrams,
   sort-key owners and relation owners still merge in the coordinator; the
   explicit bounded-wave barrier is not implemented.
3. R19: matched native/stable distributions exist only for candidate point
   lookups; native matched point/range/length/state/FTS/sort/relation
   distributions are missing.
4. R18 minor: some unavailable diagnostics still serialize as zero rather than
   null; current memory statistics for loaded components are still sourced from
   build/saved metadata rather than live capacity inspection.

No complete/near-parity claim is made. See `findings-disposition.md` and
`experiment-results.md`.
