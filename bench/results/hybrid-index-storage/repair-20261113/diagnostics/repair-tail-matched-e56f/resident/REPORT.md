# Musenalm query checkpoint

Code revision: `e56f6654e9fb93a634f1c5ce1835689a78b864c2`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6505.205 | 8843.275 | 10492.234 | 11835.527 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | ascii-off | 5.763 | 6.140 | 17.174 | 54.941 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | ascii-off | 25.864 | 27.607 | 39.136 | 100.758 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | ascii-off | 3236.082 | 3905.322 | 5638.651 | 6258.829 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 3768.526 | 4257.051 | 7155.475 | 7360.219 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 3352.090 | 3593.595 | 7372.471 | 7539.975 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 3083.805 | 3392.884 | 6151.495 | 7916.961 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | ascii-off | 181.320 | 247.890 | 1076.873 | 1805.058 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 108035.784 | 113218.455 | 137999.187 | 138983.942 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |
| Q001 | unicode-off | 6242.326 | 6889.990 | 8096.495 | 10795.013 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | unicode-off | 5.668 | 6.272 | 24.582 | 86.449 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | unicode-off | 26.474 | 27.375 | 37.928 | 104.925 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | unicode-off | 3174.046 | 3490.652 | 3978.115 | 4526.302 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | unicode-off | 3695.908 | 4009.831 | 6506.063 | 7042.114 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | unicode-off | 3380.338 | 3465.622 | 4834.460 | 4961.223 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | unicode-off | 3036.574 | 3181.086 | 3595.870 | 3974.083 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | unicode-off | 179.205 | 220.860 | 289.052 | 430.605 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | unicode-off | 107003.930 | 111270.083 | 118818.093 | 121735.785 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("target/hybrid-index-storage/repair-stores-20261113"), residency: FullyResident } open/build 0.507 s; parses 0; resident 317 MiB; peak resident 368 MiB; managed files 122 MiB.
- "unicode": IndexStoragePolicy { location: CustomDirectory("target/hybrid-index-storage/repair-stores-20261113"), residency: FullyResident } open/build 0.481 s; parses 0; resident 399 MiB; peak resident 424 MiB; managed files 125 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
