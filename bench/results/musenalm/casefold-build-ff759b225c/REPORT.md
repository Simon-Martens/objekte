# Musenalm fresh index builds

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 11.958 | 0.271 | 11.557 | 0.127 | 0.000 | 236.5 | 236.5 | 57.1 | 0.0 | — |
| ascii-rebuild | 12.260 | 0.265 | 11.580 | 0.126 | 0.275 | 250.6 | 339.4 | 57.1 | 0.0 | 106.1 |
| unicode-disabled | 12.165 | 0.279 | 11.743 | 0.128 | 0.000 | 242.2 | 242.2 | 57.1 | 0.0 | — |
| unicode-rebuild | 12.401 | 0.268 | 11.701 | 0.127 | 0.302 | 256.5 | 346.9 | 57.1 | 0.0 | 108.5 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/controlled-repair-build-20261604be`. Failures: 0.
