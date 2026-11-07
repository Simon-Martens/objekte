# Musenalm fresh index builds

Code revision: `92cdb92b19`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Build s | Snapshot write s | Peak RSS MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|
| ascii-disabled | 16.861 | 16.861 | 0.000 | 236.2 | — |
| ascii-rebuild | 17.319 | 16.780 | 0.351 | 339.5 | 106.0 |
| unicode-disabled | 17.916 | 17.916 | 0.000 | 257.7 | — |
| unicode-rebuild | 18.316 | 17.908 | 0.453 | 370.0 | 116.7 |

Baseline: `/home/simon/source/objekte/bench/baselines/musenalm-build.json`. Failures: 0.
