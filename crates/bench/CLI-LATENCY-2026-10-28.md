# CLI latency with the application cache on disk

Follow-up to the [metadata-manifest measurements](METADATA-MANIFEST-2026-10-28.md),
28 October 2026. The earlier cache was on `/tmp` (tmpfs). Here both the cache and
Musenalm source files are on Btrfs, matching the user's normal application-data
filesystem. The existing application cache was copied under `target/` while holding
its shared lock; the original cache and source JSON were left unchanged.

Each sample starts a fresh CLI process and returns the default 50-row view. Tables
below show medians of three processes per build/collection/version. Compilation
and Cargo startup are excluded. OS filesystem caches are warm and uncontrolled;
these are not measurements of a cold disk. Full returned rows and view fingerprints
match before and after. Every repeated run reports one manifest hit, zero source
reads, and zero index builds; the existing format-2 cache remains usable.

| Collection | Build | Before | After |
| --- | --- | ---: | ---: |
| `contents` (133,948 files) | Debug | 1.706 s | 1.466 s |
| `contents` | Release | 0.531 s | 0.374 s |
| `entries` (4,928 files) | Debug | 0.197 s | 0.137 s |
| `entries` | Release | 0.090 s | 0.032 s |

## Changes and remaining costs

- Cache opening used to rewrite three metadata rows and all configuration hashes
  on every process start, even when identical. With full SQLite durability on disk,
  those unnecessary commits cost about 40–47 ms. Conditional updates now avoid
  changed-page commits for unchanged values; opening takes about 0.8–1.5 ms.
- Linux inventory now obtains file metadata relative to the pinned directory
  descriptor. It retains no-follow checks and the same size, time, and identity
  stamps, while avoiding repeated resolution of the full parent path.
- JSON serialization into SHA-256 is buffered in 64 KiB chunks. Persisted hash
  bytes are identical. Debug manifest hashing improves modestly; optimized hashing
  was already fast. No build-profile settings were changed.

| `contents` phase (median) | Debug before | Debug after | Release before | Release after |
| --- | ---: | ---: | ---: | ---: |
| Metadata inventory | 723.7 ms | 673.7 ms | 429.3 ms | 335.0 ms |
| Manifest fingerprint | 876.2 ms | 746.6 ms | 19.3 ms | 19.4 ms |
| Cache open | 45.3 ms | 1.3 ms | 40.0 ms | 0.9 ms |
| Query and row construction | 3.0 ms | 3.1 ms | 1.3 ms | 1.3 ms |

Phase medians need not add to the median process time. `sync_nanos` is a parent
timer containing inventory, fingerprinting, and reconciliation; do not add it to
its children. Total process time also includes configuration loading and output.

The CLI still checks every filename and stamp once per invocation before returning
current rows. This is O(number of files), even on a cache hit. A manifest hash
avoids individual-record database comparisons and source reads; it does not avoid
constructing the manifest. Removing this wait from first paint belongs to the
deferred [UI optimistic-delivery work](../../docs/UI.md).

## Initial cache construction

Separate empty disk-cache directories were used for these single-run observations
with the updated implementation. Source OS caches remained warm; "initial" means
no application cache, not an uncached disk. These are not before/after comparisons.

| Collection | Debug | Release |
| --- | ---: | ---: |
| `contents` | 35.886 s | 9.201 s |
| `entries` | 17.187 s | 4.077 s |

Initial construction reads, hashes, validates, extracts relationships, stores JSON
and typed values, and builds indexes for the entire requested collection before
returning its default page. For `contents`, debug content hashing takes 4.00 s and
parsing/validation 9.82 s; in release those phases take 0.09 s and 1.53 s. SQLite
inserts, relationship extraction/writes, index work, and durable publication also
contribute. The current timers do not separate all of those remaining costs, so
the full remainder cannot be attributed to index construction alone.

`entries` has fewer files but large nested records, so file count alone does not
predict its initial cost. These changes primarily address repeated startup;
initial import remains significant. A future first-import optimization should
profile the remaining write/extraction/commit costs separately.

## Reproduce

```sh
cargo build -p objekte-cli
cargo build -p objekte-cli --release
target/release/objekte examples/musenalm view contents --data-dir target/disk-cache-profile --diagnostics
target/debug/objekte examples/musenalm view contents --data-dir target/disk-cache-profile --diagnostics
target/release/objekte examples/musenalm view contents --data-dir target/disk-cache-profile --diagnostics
target/release/objekte examples/musenalm view entries --data-dir target/disk-cache-profile --diagnostics
```

If the chosen cache is empty, the first call builds it and must be excluded from
unchanged-cache timings. Repeat each command in a new process three times. Both
builds use the same compatible cache. For the user's usual invocation:

```sh
cargo run --release -p objekte-cli -- examples/musenalm view contents --diagnostics
```

Raw local reports (ignored): `target/cli-latency-profile/before.json`, `after.json`,
and `cold-after.json`.

## Verification

All 88 regular core/CLI tests and Clippy passed. The complete Musenalm acceptance
passed in 75.53 s, comparing all 148,486 default-view records with direct execution,
numeric/filter queries, secondary sorting, relationships, and metadata/strict
reopening. Its separate report is `target/musenalm-cli-latency-acceptance.json`.
The focused tests check fingerprint byte compatibility, pre-epoch timestamps,
symlink rejection, and unchanged SQLite `data_version` across cache reopen.
