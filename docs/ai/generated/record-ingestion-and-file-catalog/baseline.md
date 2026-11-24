# Record ingestion baseline, 2026-11-24

Task: `record-ingestion-and-file-catalog`. Measured source: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Integration branch: `integration/record-ingestion-and-file-catalog`. The base product code was unchanged. The only file added to the detached baseline worktree is the same measurement-only example stored in this branch.

The physical source fixture was `/home/simon/source/objekte/examples/musenalm` (148,498 JSON files including configuration; 148,486 indexed records). Content fingerprint: `79acbab9155aa95bc869afde56dc1b402e5a09f9c6a80a5106fc665e2c3f20c5`; metadata fingerprint: `b0a47239de14ad78c0aa059c3c1a6e48f3d988ae234d6c904fc78c3a96429106`. The separate 16,384-file probe fixture is `/home/simon/source/objekte-record-ingestion-probe-fixture`; its fingerprint is in [before/validation.json](before/validation.json). Both fixture paths must remain unchanged for the final pair.

Build command: `cargo build --locked --release -p objekte-search-core --example bench_build_musenalm --example bench_musenalm --example bench_lifecycle_mutations` in the detached baseline worktree, using its own `target/`. The measurement-only `bench_record_ingestion_gate` example was built separately with the same release profile. The benchmark driver used the release binaries directly. Measured toolchain was `rustc 1.99.1`, `cargo 1.99.1`, CPU affinity 0–31, performance governor, 32 build workers, one query worker and the default release allocator/settings. The initial interactive login shell resolved a different toolchain; all measured build and run commands used the non-login environment. Full tool, binary, lockfile and workload hashes are in [before/manifest.json](before/manifest.json).

| Baseline metric | Result |
| --- | ---: |
| ASCII build wall median, three processes | 627,615,878 ns |
| ASCII build peak RSS median | 599,928,832 bytes |
| Unicode build wall median, three processes | 706,673,064 ns |
| Unicode build peak RSS median | 652,345,344 bytes |
| ASCII nine-query median sum | 118,167,665 ns |
| Unicode nine-query median sum | 118,081,011 ns |
| Direct ID lookup median | 149.35 ns per operation |
| Projected residual median | 485,862 ns per query |

All 14 required build, query and lifecycle invocations exited successfully. Both historical profiles returned 150 successful rows, both focused profiles nine successful seven-sample rows, and the extended Unicode profile six successful seven-sample rows. Canonical historical semantic SHA-256 was `b06b294ee77503794be1ed1fb97e5f67480d864e7deea689a8ad1a36aea8aee5` under both alphabets; the six extended cases produced `79d3f1a6e964520e6cb3878a6256009ca8c373cfa46caf2a2801438b97bff3c6`. The controlled year count was 1,024 with zero source opens and bytes read. Direct and residual probe results passed the expected counts. [before/checksums.sha256](before/checksums.sha256) covers the committed raw files. Disposable mutation fixture contents are excluded from Git.

The [file coupling inventory](file-coupling-inventory.txt) records 1,663 source locations across core, CLI, desktop, examples and integration tests for mandatory file fields, catalog access and verification dispatch. In the implementation base, no reachable on-disk index snapshot serializer is present. `bincode` in core production code is used for a verified-result cache key; other found uses are tests and benchmark output. The new spool must have its own explicit format, while current file-opening behavior remains the compatibility baseline.
