# Musenalm fresh index builds

Code revision: `4f2cb8e85ba6b1a6fe6f5b7aecc8f0c7fff1aee8`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 5.438 | 0.259 | 5.062 | 0.120 | 0.000 | 237.6 | 237.6 | 57.1 | 0.0 | — |
| ascii-rebuild | 5.700 | 0.257 | 5.047 | 0.119 | 0.255 | 251.6 | 340.7 | 57.1 | 0.0 | 106.1 |
| unicode-disabled | 6.233 | 0.261 | 5.846 | 0.119 | 0.000 | 243.1 | 243.1 | 57.1 | 0.0 | — |
| unicode-rebuild | 6.525 | 0.261 | 5.855 | 0.120 | 0.269 | 257.5 | 347.9 | 57.1 | 0.0 | 108.5 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/casefold-build-ff759b225c`. Failures: 0.
