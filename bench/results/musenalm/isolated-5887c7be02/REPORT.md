# Isolated archived-core comparison

Revision: `5887c7be021cffb013a7e6324a80a754c460a355`. All 34 exact counts and ordered 50-record pages match.
The old and new cores load the same seven Musenalm schemas and exactly the same 89 indexed paths.
Both use ASCII bigrams, exact counts, logical payloads, ID order, and disabled verified query caches.
Each core runs alone in a process twice, in old/new/new/old order, with five timed repetitions per process.
Index loading and warmup occur before query timing. The old core uses Memory repository mode; the new core uses 32 query workers.
Median old/new ratio across cases: 6.41×. Old faster: 1; new faster: 33.

| Case | Matches | Old ms | New ms | Old / new |
|---|---:|---:|---:|---:|
| Q031 | 203 | 1.597 | 0.266 | 5.99× |
| Q032 | 7 | 0.506 | 0.041 | 12.43× |
| Q037 | 753 | 3.100 | 0.460 | 6.73× |
| Q038 | 3257 | 17.956 | 4.378 | 4.10× |
| Q042 | 1520 | 17.389 | 5.088 | 3.42× |
| Q043 | 3022 | 15.054 | 6.030 | 2.50× |
| Q044 | 60 | 3.280 | 1.366 | 2.40× |
| Q045 | 1107 | 9.260 | 1.405 | 6.59× |
| Q046 | 3308 | 12.130 | 1.948 | 6.23× |
| Q049 | 4702 | 14.593 | 1.481 | 9.85× |
| Q050 | 81 | 4.026 | 1.328 | 3.03× |
| Q052 | 86764 | 63.952 | 5.512 | 11.60× |
| Q053 | 2387 | 6.775 | 1.373 | 4.94× |
| Q055 | 112778 | 70.743 | 6.318 | 11.20× |
| Q057 | 4177 | 5.189 | 0.605 | 8.58× |
| Q058 | 0 | 0.378 | 0.119 | 3.18× |
| Q059 | 4177 | 14.071 | 0.618 | 22.77× |
| Q060 | 1187 | 4.144 | 0.449 | 9.22× |
| Q061 | 1 | 1.082 | 0.013 | 85.90× |
| Q062 | 3 | 0.639 | 0.018 | 34.67× |
| Q065 | 207 | 2.291 | 0.440 | 5.21× |
| Q071 | 1003 | 8.496 | 2.068 | 4.11× |
| Q074 | 4927 | 10.562 | 5.419 | 1.95× |
| Q081 | 888 | 8.734 | 2.304 | 3.79× |
| Q084 | 4922 | 12.823 | 1.472 | 8.71× |
| Q088 | 5 | 3.116 | 0.217 | 14.35× |
| Q093 | 3856 | 10.947 | 1.541 | 7.10× |
| Q096 | 70390 | 57.131 | 4.280 | 13.35× |
| Q101 | 1 | 1.199 | 0.044 | 27.56× |
| Q103 | 4108 | 6.625 | 0.608 | 10.90× |
| Q113 | 951 | 8.117 | 4.137 | 1.96× |
| Q117 | 63520 | 84.783 | 86.177 | 0.98× |
| Q120 | 42700 | 82.095 | 81.406 | 1.01× |
| Q121 | 341 | 3.004 | 1.041 | 2.89× |

Raw process outputs and exact sample durations are retained in this directory.
