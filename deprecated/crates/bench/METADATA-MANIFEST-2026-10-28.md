# Metadata-manifest reconciliation and view fingerprints

Measured on 28 October 2026 using the current library and real Musenalm `contents`
(133,948 files). Each sample starts a new CLI process and returns the default
50-row title view. Full returned rows were checked for equality across samples.
Commands ran directly from `target/{debug,release}/objekte`, excluding compilation
and Cargo overhead. The cache was a fresh temporary directory on `/tmp` (tmpfs);
source files were on Btrfs with warm, uncontrolled OS caches. No source edits.

| Operation | Build | Time | Samples |
| --- | --- | ---: | ---: |
| Initial cache construction | Release | 7.095 s | 1 |
| Unchanged reopen, metadata policy | Release | **0.463 s** | median of 3 |
| Unchanged reopen, strict content policy | Release | 2.355 s | median of 3 |
| Unchanged reopen, metadata policy | Debug | **1.640 s** | median of 3 |
| Unchanged reopen, strict content policy | Debug | 8.153 s | 1 |

The previous [library profile](LIBRARY-PROFILE-2026-10-28.md) measured approximately
3.6 s release / 8.4 s debug for unchanged CLI opens under full hashing. Default
metadata reconciliation now takes about one eighth of that release latency; this
is a different, explicitly weaker freshness policy, not equivalent verification
made eight times faster. Strict mode also improves after removing repeated path
resolution and using descriptor reads, from roughly 3.6 s to 2.36 s in release.

Every unchanged metadata sample recorded:

- One directory inventory, one compound manifest hit.
- Zero record-file reads, content hashes, source parses, validations, or index writes.
- Zero loads of the individual-record SQLite manifest.
- 133,948 cached records reused.

One representative release sample spent 426 ms collecting metadata, 19 ms hashing
the manifest, and 1.2 ms executing the page query and constructing its rows. The
remaining startup cost is largely filesystem metadata access. SHA-256 uses a
streamed, domain-tagged structured manifest; debug fingerprinting is substantially
slower than optimized code.

## Implemented contract

The default is `metadata-on-open`. It fingerprints sorted filenames and stamps
(size, nanosecond mtime, plus Unix device/inode/ctime). An unchanged complete cache
with compatible schema/mapping/index fingerprints is reused. Otherwise individual
entries select new/changed candidates for content hashing, while removed records
are deleted. Metadata changes with unchanged contents update stamps without
reparsing or reindexing. Dirty/partial state forces complete recovery. Metadata
equality is not proof of identical contents on every filesystem; `hash-on-open`
explicitly hashes all source bytes. Both policies have `*-on-request` variants.

View-definition and resolved-request hashes are separate from source-manifest,
schema, mapping, and typed-index hashes. They include filters, sort directions and
ordered secondary keys, projection, and pagination/count settings as appropriate.
Equivalent resolved requests share hashes; changing a sub-sort changes the sort
hash. Changing only view settings does not force source reads or index rebuilding
when the required typed fields already exist. Fingerprints identify definitions;
they do not implement query-result caches or automatic compound SQL indexes.

Candidate reads use a pinned directory descriptor on Unix, reject symlinks, and
compare opened-file metadata before/after reads. Metadata scans also check directory
identity/membership stability. Strict mode retains the final whole-file inventory
check. Neither mode promises an atomic snapshot of concurrently edited source files.

Cache format 2 rebuilds previous disposable databases once. Performance measurements
after that rebuild are the relevant unchanged-cache comparison.

## Reproduce

```sh
cargo build -p objekte-cli --release
target/release/objekte examples/musenalm view contents --data-dir /tmp/objekte-manifest --diagnostics
target/release/objekte examples/musenalm view contents --data-dir /tmp/objekte-manifest --diagnostics
target/release/objekte examples/musenalm view contents --data-dir /tmp/objekte-manifest --reconcile hash-on-open --diagnostics
target/release/objekte examples/musenalm view contents --data-dir /tmp/objekte-manifest --sort year --then-sort numbering:desc --output json
```

UI defaults can set `list.subSorts` to an ordered array of `{field, direction}`.
The CLI accepts repeated `--then-sort FIELD:asc|desc`. JSON view output and
`--diagnostics` expose `view_hashes`; cache status exposes saved manifest/view hashes.

Raw measurements: `target/metadata-manifest-profile-2026-10-28.json` (ignored).
The focused suites cover targeted updates, fingerprint invalidation, strict mode,
sub-sort parity, transaction recovery, and directory/symlink replacement. The full
Musenalm acceptance also checks metadata and strict reopen plus a compound sort.
