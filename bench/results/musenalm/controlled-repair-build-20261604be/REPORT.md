# Musenalm fresh index builds

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 11.343 | 0.261 | 10.958 | 0.122 | 0.000 | 236.4 | 236.4 | 57.1 | 0.0 | — |
| ascii-rebuild | 11.842 | 0.266 | 11.190 | 0.120 | 0.270 | 250.5 | 339.4 | 57.1 | 0.0 | 106.0 |
| unicode-disabled | 12.089 | 0.260 | 11.707 | 0.121 | 0.000 | 257.8 | 257.8 | 57.1 | 0.0 | — |
| unicode-rebuild | 12.647 | 0.260 | 11.961 | 0.122 | 0.305 | 273.5 | 370.4 | 57.1 | 0.0 | 116.7 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/controlled-foundation-build-performance-92cdb92b19`. Failures: 0.
