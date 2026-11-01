# Default modification-time order benchmark

Compared commit `1200ba6815` (the previous default ID-order path, with no
explicit `SORT BY`) against the new default modification-time order. Both
versions ran in release mode on the same 148,486-file Musenalm example. Each
benchmark warmed its indexes and executed every query once before timing.
Match counts agreed for every case. Index warm-up time is excluded because
switching between secondary-cache versions forces a one-time rebuild.

| Workload | Runs per case | Previous sum of case medians | New sum | Change | Previous median case | New median case |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 JSON filters across seven collections | 4 | 1,566.9 ms | 1,521.8 ms | -2.9% | 9.93 ms | 10.64 ms |
| 30 full-text and `:fts` cases in `contents` | 3 | 1,066.7 ms | 1,080.4 ms | +1.3% | 14.68 ms | 15.36 ms |

The small differences are within the variation of these short local runs.
The new order did not cause a material query-time regression on this example.
The benchmark covers 148,486 files, so it does not establish a result for a
five-million-file repository.

Commands (run once for each version, using its own compiled binary):

```sh
json-query-mix --repo examples/musenalm --workload crates/bench/workloads/musenalm-mix-50.json --runs 4
fts-query-mix examples/musenalm 3
```
