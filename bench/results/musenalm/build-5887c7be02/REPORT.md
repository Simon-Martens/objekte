# Musenalm fresh index builds

Code revision: `5887c7be02`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Build s | Snapshot write s | Peak RSS MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|
| ascii-disabled | 14.222 | 14.222 | 0.000 | 226.8 | — |
| ascii-rebuild | 14.530 | 14.260 | 0.270 | 323.6 | 101.8 |
| unicode-disabled | 15.447 | 15.447 | 0.000 | 248.3 | — |
| unicode-rebuild | 15.840 | 15.541 | 0.300 | 354.4 | 112.5 |

Baseline: `/home/simon/source/objekte/bench/baselines/musenalm-build.json`. Failures: 0.
