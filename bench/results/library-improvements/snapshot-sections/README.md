# R3 snapshot section assessment

The current snapshot format remains unchanged. This is one explicit save and
one fresh-process restore on an ASCII Musenalm repository. It measures the
present limit and phases; it does not compare an optimization.

## Reproduction

- Core code: `bde4c77813cc267fce1b56e8286c7363aa8895da`; diagnostic example
  commit: `be798e00ea287229ffadfa794e5b8272512270b4`. The example change
  only prints the existing `SnapshotSaveStats` fields. Its release binary was
  compiled from that code before the diagnostic commit.
- Toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, release profile, x86_64,
  Intel Core i9-13900K, 32 available and resolved build workers.
- Source: a `cp -a` copy of this worktree's `examples/musenalm` at
  `target/r3-snapshot/musenalm`. The snapshot was written only into the copy.
  Seven collection schemas have manifest digest
  `38f2dc5112146aa6ac0514291c76eaeb741421fa8a4624e50fa83c4c0a449bf9`.
  The fresh index signature is
  `351ca09263876693a893aeec797c7bdc6216fb1b212b3b6c2460b36227392fb3`.
  There were 148,486 records, 148,486 successful build parses and no failed
  parses. Snapshots were disabled during opening; the save was explicit.
- `bench_build_musenalm` release binary SHA-256:
  `742249eba3830163785e297eb076de8f931b359ed16f12615acd6c67a72782e9`.
  `bench_musenalm` release binary SHA-256:
  `1b7c1edcc9a6646c7da9206dd9e575245d0aecadb6ac77da3a5e63234778c73e`.

After compiling the two examples, the timed invocations were:

```sh
./target/release/examples/bench_build_musenalm \
  --root target/r3-snapshot/musenalm --alphabet ascii \
  --snapshot-mode rebuild --build-workers auto \
  --schema-manifest-path target/r3-snapshot/schema.json \
  > target/r3-snapshot/build-save.json \
  2> target/r3-snapshot/build-save.stderr.log
./target/release/examples/bench_musenalm \
  --restore-child target/r3-snapshot/musenalm ascii \
  > target/r3-snapshot/restore.json \
  2> target/r3-snapshot/restore.stderr.log
```

The copied raw JSON, schema manifest and stderr logs are beside this file.
Build/open and explicit save are separate timing intervals. The restore is a
new process against the saved snapshot. No query workload was run here.

## Observation

| Phase | Time (ms) |
| --- | ---: |
| Fresh repository opening/build | 1,183.035 |
| Explicit save, total | 488.892 |
| Inventory encoding preparation | 9.517 |
| Section serialization, summed | 97.898 |
| Section checksums, summed | 41.843 |
| Section/header writes | 248.154 |
| `sync_all` | 81.899 |
| Fresh-process restore, total | 469.271 |
| Restore decode | 257.567 |
| Restore source reconciliation | 210.100 |

The snapshot is 111,255,130 bytes (106.101 MiB). Its largest collection
section is 71,232,441 bytes (67.933 MiB), 13.3% of the current 512 MiB
per-section limit. The example reported 500,625,408 resident bytes and
590,716,928 peak resident bytes after saving; these are process measurements,
not the snapshot buffer allocation. Restore reported zero build parses.

This is one warm-filesystem-cache sample. The substantial write and sync time
tracks the 106 MiB of bytes written; it does not establish that serialization
is inefficient or that a different section format would be faster.

## Decision

Defer a versioned chunked snapshot format. The measured largest real section
has more than sevenfold headroom, and no supported over-512 MiB collection
workload was supplied for this cycle. A streaming/chunked format would change
restore compatibility and require separate large-scale, corruption and failed
save validation. The present writer still buffers each entire section before
checking its size and cannot explicitly save a section above 512 MiB. This
assessment does not claim otherwise or provide the conditional large-section
acceptance test.
