# Query optimization and archived-core comparison

The optimized core revision `128a7601c2` passes the independent 150-case `jq` oracle under both alphabets and the full 3,000-sample regression run. The quick and full latency gates now use the optimized run as their pinned baseline.

The prior core used one query worker. The optimized core uses a bounded pool of 32 workers for independent source files and avoids retaining logical payloads for matches that cannot reach the requested page. Both runs use the same Musenalm source, schemas, workload, and cache profiles.

| Cold profile | Prior sum of case medians | Optimized sum | Speedup |
|---|---:|---:|---:|
| ASCII, verified cache off | 26.234 s | 2.792 s | 9.40× |
| Unicode, verified cache off | 23.124 s | 2.489 s | 9.29× |

For broad content scans, Q117 fell from 1,991 ms to 179 ms and Q120 from 1,999 ms to 180 ms in the ASCII cold profile. These are sums and medians of per-query measurements, not end-to-end command wall times. Warm verified-result profiles take about 2 ms for all 150 case medians and are dominated by microsecond-scale timing noise.

In the [isolated archived-core comparison](../isolated-128a7601c2/REPORT.md), both cores load all seven schemas and exactly the same 89 indexed paths. Exact counts and ordered 50-record pages match for all 34 shared cases. The new core is faster on 31; its median old/new ratio is 5.49×. The archived core remains faster on the two broad scans Q117 and Q120 by roughly 2×, which is the main remaining performance gap.

Source measurements: [prior full run](../final-8dd0c2aa6d/query-results.csv), [optimized full run](../full-128a7601c2/query-results.csv), [isolated comparison](../isolated-128a7601c2/timings.csv).
