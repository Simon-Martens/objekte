# Musenalm fresh index builds

Code revision: `fdf4c14822`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Build s | Snapshot write s | Peak RSS MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|
| ascii-disabled | 14.216 | 14.216 | 0.000 | 227.0 | — |
| ascii-rebuild | 15.343 | 15.073 | 0.270 | 323.6 | 101.8 |
| unicode-disabled | 15.849 | 15.849 | 0.000 | 248.3 | — |
| unicode-rebuild | 16.134 | 15.782 | 0.327 | 354.4 | 112.5 |

Baseline: `/home/simon/source/objekte/bench/baselines/musenalm-build.json`. Failures: 0.

