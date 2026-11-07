# Musenalm fresh index builds

Code revision: `dcba315849a4b845bf6e207c73934d2bff0731b4`. Each configuration ran in 3 separate processes.
Both alphabets were measured with snapshot modes: disabled.
All builds parsed exactly 148,486 source records; medians include discovery, build and optional snapshot write.

| Configuration | Workers | Wall s | Range s | CPU s | Busy CPUs | Discovery s | Extraction/index s | Finalization s | Snapshot write s | Resident MiB | Peak RSS MiB | Peak admitted MiB | Snapshot MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| ascii-disabled | 1 | 6.000 | 5.995–6.020 | 7.135 | 1.19 | 0.311 | 5.566 | 0.112 | 0.000 | 267.5 | 278.4 | 16.4 | — |
| unicode-disabled | 1 | 6.823 | 6.771–7.079 | 7.987 | 1.17 | 0.314 | 6.382 | 0.116 | 0.000 | 269.3 | 279.6 | 16.4 | — |

Baseline: `/home/simon/source/objekte/bench/results/musenalm/parallel-index-construction-d15b689d1a/one`. Failures: 0.
