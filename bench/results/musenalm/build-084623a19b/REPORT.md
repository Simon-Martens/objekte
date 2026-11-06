# Musenalm fresh index builds

Code revision: `084623a19b`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Build s | Snapshot write s | Peak RSS MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|
| ascii-disabled | 14.549 | 14.549 | 0.000 | 229.2 | — |
| ascii-rebuild | 14.848 | 14.531 | 0.299 | 327.8 | 103.4 |
| unicode-disabled | 15.741 | 15.741 | 0.000 | 250.6 | — |
| unicode-rebuild | 16.139 | 15.816 | 0.323 | 358.8 | 114.1 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/build-5887c7be02/gate.json`. Failures: 0.
