# Musenalm fresh index builds

Code revision: `d15b689d1ac21709195b79fc382c045d8dec9867`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshot modes: disabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Workers | Wall s | Range s | CPU s | Busy CPUs | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Peak admitted MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 2 | 3.468 | 3.450–3.510 | 6.580 | 1.90 | 0.315 | 3.027 | 0.117 | 0.000 | 266.1 | 276.6 | 19.9 | — |
| unicode-disabled | 2 | 3.895 | 3.892–3.903 | 7.434 | 1.90 | 0.309 | 3.462 | 0.115 | 0.000 | 271.8 | 282.7 | 19.9 | — |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/parallel-bigram-4f2cb8e85b`. Failures: 0.
