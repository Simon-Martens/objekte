# Musenalm fresh index builds

Code revision: `c27e68b315a308540a7c82f16b985a7fef80888e`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 1.705 | 0.379 | 1.173 | 0.111 | 0.000 | 316.1 | 334.3 | 57.1 | 0.0 | — |
| ascii-rebuild | 1.986 | 0.308 | 1.146 | 0.113 | 0.373 | 342.9 | 406.4 | 57.1 | 0.0 | 106.1 |
| unicode-disabled | 1.762 | 0.392 | 1.233 | 0.119 | 0.000 | 320.0 | 338.7 | 57.1 | 0.0 | — |
| unicode-rebuild | 2.157 | 0.309 | 1.254 | 0.115 | 0.453 | 360.2 | 425.2 | 57.1 | 0.0 | 108.5 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/parallel-bigram-4f2cb8e85b`. Failures: 2.

- ascii-disabled: peak RSS exceeds baseline
- unicode-disabled: peak RSS exceeds baseline
