# Musenalm fresh index builds

Code revision: `954b812c337042de5bcecb091df124990fc65512`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshots disabled and with snapshot rebuilding enabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Wall s | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Inventory MiB | Diagnostics KiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 1.575 | 0.324 | 1.128 | 0.110 | 0.000 | 277.2 | 287.4 | 57.1 | 0.0 | — |
| ascii-rebuild | 1.947 | 0.310 | 1.161 | 0.118 | 0.363 | 287.7 | 374.4 | 57.1 | 0.0 | 106.1 |
| unicode-disabled | 1.747 | 0.378 | 1.269 | 0.116 | 0.000 | 278.3 | 287.8 | 57.1 | 0.0 | — |
| unicode-rebuild | 2.044 | 0.310 | 1.256 | 0.110 | 0.379 | 295.1 | 384.0 | 57.1 | 0.0 | 108.5 |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/parallel-bigram-4f2cb8e85b`. Failures: 0.
