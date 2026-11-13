# Musenalm query checkpoint

Code revision: `e56f6654e9fb93a634f1c5ce1835689a78b864c2`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6544.398 | 8896.712 | 13502.968 | 13762.037 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | ascii-off | 6.500 | 6.967 | 13.280 | 57.624 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | ascii-off | 26.361 | 29.412 | 38.403 | 95.958 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | ascii-off | 3208.426 | 3901.083 | 5368.972 | 6687.897 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 3726.804 | 4320.846 | 6438.227 | 7257.468 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 3271.659 | 3675.597 | 6882.116 | 9172.303 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 3068.682 | 3419.930 | 5532.994 | 5985.419 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | ascii-off | 197.114 | 245.451 | 1737.814 | 2544.259 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 109366.590 | 113752.142 | 137155.364 | 137452.250 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |
| Q001 | unicode-off | 6167.467 | 6815.687 | 7584.277 | 10298.299 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | unicode-off | 10.758 | 11.076 | 16.014 | 79.426 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | unicode-off | 44.292 | 45.939 | 55.688 | 129.985 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | unicode-off | 3126.537 | 3461.663 | 4870.552 | 5064.090 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | unicode-off | 3616.200 | 3870.130 | 5451.504 | 7503.219 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | unicode-off | 3319.668 | 3516.279 | 4965.926 | 4995.806 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | unicode-off | 3048.586 | 3224.690 | 4236.170 | 4569.169 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | unicode-off | 183.844 | 205.525 | 281.089 | 480.418 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | unicode-off | 106129.743 | 110662.416 | 113322.911 | 114368.972 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("target/hybrid-index-storage/repair-stores-20261113"), residency: OnDemand } open/build 0.396 s; parses 0; resident 264 MiB; peak resident 316 MiB; managed files 122 MiB.
- "unicode": IndexStoragePolicy { location: CustomDirectory("target/hybrid-index-storage/repair-stores-20261113"), residency: OnDemand } open/build 0.334 s; parses 0; resident 345 MiB; peak resident 345 MiB; managed files 125 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
