# Musenalm fresh index builds

Code revision: `9b8aa7131705d755c8ccebeefd7b9edf596ce9b2`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 11.758 | 0.282 | 11.368 | 0.127 | 0.000 | 236.5 | 236.5 | 57.1 | 0.0 | — |
| ascii-rebuild | 12.122 | 0.261 | 11.457 | 0.124 | 0.284 | 250.5 | 339.2 | 57.1 | 0.0 | 106.0 |
| unicode-disabled | 13.162 | 0.261 | 12.776 | 0.124 | 0.000 | 257.9 | 257.9 | 57.1 | 0.0 | — |
| unicode-rebuild | 13.172 | 0.261 | 12.417 | 0.133 | 0.343 | 273.6 | 370.2 | 57.1 | 0.0 | 116.7 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/build-92cdb92b19`. Failures: 0.
