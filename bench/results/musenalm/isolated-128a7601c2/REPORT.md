# Isolated archived-core comparison

Revision: `128a7601c2defec3fd9f5dbb851fb2a4303f7f9a`. All 34 exact counts and ordered 50-record pages match.
The old and new cores load the same seven Musenalm schemas and exactly the same 89 indexed paths.
Both use ASCII bigrams, exact counts, logical payloads, ID order, and disabled verified query caches.
Each core runs alone in a process twice, in old/new/new/old order, with five timed repetitions per process.
Index loading and warmup occur before query timing. The old core uses Memory repository mode; the new core uses 32 query workers.
Median old/new ratio across cases: 5.49×. Old faster: 3; new faster: 31.

| Case | Matches | Old ms | New ms | Old / new |
|---|---:|---:|---:|---:|
| Q031 | 203 | 2.070 | 0.355 | 5.83× |
| Q032 | 7 | 1.345 | 0.053 | 25.15× |
| Q037 | 753 | 6.025 | 0.544 | 11.08× |
| Q038 | 3257 | 20.971 | 4.493 | 4.67× |
| Q042 | 1520 | 18.168 | 5.219 | 3.48× |
| Q043 | 3022 | 17.405 | 6.152 | 2.83× |
| Q044 | 60 | 3.972 | 1.453 | 2.73× |
| Q045 | 1107 | 7.678 | 1.485 | 5.17× |
| Q046 | 3308 | 9.710 | 2.054 | 4.73× |
| Q049 | 4702 | 13.472 | 1.563 | 8.62× |
| Q050 | 81 | 3.427 | 1.339 | 2.56× |
| Q052 | 86764 | 67.687 | 5.602 | 12.08× |
| Q053 | 2387 | 5.949 | 1.490 | 3.99× |
| Q055 | 112778 | 74.889 | 6.557 | 11.42× |
| Q057 | 4177 | 4.040 | 0.695 | 5.81× |
| Q058 | 0 | 0.380 | 0.121 | 3.15× |
| Q059 | 4177 | 10.372 | 0.698 | 14.85× |
| Q060 | 1187 | 3.457 | 0.525 | 6.59× |
| Q061 | 1 | 1.145 | 0.015 | 74.45× |
| Q062 | 3 | 0.508 | 0.026 | 19.55× |
| Q065 | 207 | 1.785 | 0.496 | 3.60× |
| Q071 | 1003 | 9.336 | 4.278 | 2.18× |
| Q074 | 4927 | 13.363 | 13.140 | 1.02× |
| Q081 | 888 | 5.810 | 4.234 | 1.37× |
| Q084 | 4922 | 11.149 | 1.576 | 7.08× |
| Q088 | 5 | 2.544 | 0.267 | 9.53× |
| Q093 | 3856 | 10.664 | 1.650 | 6.46× |
| Q096 | 70390 | 51.187 | 4.403 | 11.63× |
| Q101 | 1 | 1.200 | 0.046 | 26.18× |
| Q103 | 4108 | 6.575 | 0.694 | 9.47× |
| Q113 | 951 | 9.104 | 9.318 | 0.98× |
| Q117 | 63520 | 83.721 | 158.676 | 0.53× |
| Q120 | 42700 | 77.693 | 167.694 | 0.46× |
| Q121 | 341 | 2.877 | 1.388 | 2.07× |

Raw process outputs and exact sample durations are retained in this directory.
