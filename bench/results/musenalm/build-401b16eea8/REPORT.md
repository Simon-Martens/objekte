# Musenalm fresh index builds

Code revision: `401b16eea8`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Build s | Snapshot write s | Peak RSS MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|
| ascii-disabled | 14.962 | 14.962 | 0.000 | 226.8 | — |
| ascii-rebuild | 14.687 | 14.427 | 0.277 | 323.8 | 101.8 |
| unicode-disabled | 15.653 | 15.652 | 0.000 | 248.4 | — |
| unicode-rebuild | 15.856 | 15.481 | 0.326 | 354.6 | 112.5 |

Baseline: `/home/simon/source/objekte/bench/baselines/musenalm-build.json`. Failures: 0.

