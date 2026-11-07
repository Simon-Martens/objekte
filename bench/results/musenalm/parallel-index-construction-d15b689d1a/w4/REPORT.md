# Musenalm fresh index builds

Code revision: `d15b689d1ac21709195b79fc382c045d8dec9867`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshot modes: disabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Workers | Wall s | Range s | CPU s | Busy CPUs | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Peak admitted MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 4 | 1.870 | 1.865–1.909 | 7.079 | 3.78 | 0.312 | 1.426 | 0.120 | 0.000 | 272.2 | 282.9 | 51.4 | — |
| unicode-disabled | 4 | 2.084 | 2.067–2.091 | 7.920 | 3.81 | 0.308 | 1.646 | 0.115 | 0.000 | 278.0 | 288.6 | 51.4 | — |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/parallel-bigram-4f2cb8e85b`. Failures: 0.
