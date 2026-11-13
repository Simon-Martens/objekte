# Musenalm query checkpoint

Code revision: `e56f6654e9fb93a634f1c5ce1835689a78b864c2`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5559.284 | 7269.333 | 10119.029 | 10347.735 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | ascii-off | 5.832 | 6.794 | 9.743 | 50.157 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | ascii-off | 26.466 | 28.234 | 38.982 | 82.640 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | ascii-off | 2978.238 | 3354.073 | 4289.192 | 4604.260 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 3206.314 | 3627.653 | 5215.195 | 6743.192 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 2987.325 | 3183.896 | 6165.068 | 6186.745 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 2683.044 | 3093.860 | 4458.419 | 4640.794 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | ascii-off | 166.111 | 196.117 | 571.357 | 1059.552 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 86978.994 | 91934.210 | 114133.293 | 117879.421 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |
| Q001 | unicode-off | 5323.312 | 5902.646 | 7075.113 | 7802.696 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q008 | unicode-off | 6.614 | 6.836 | 9.866 | 51.011 | 0 | 0 | 0 | 0/30 | 0/30 | ok |
| Q042 | unicode-off | 26.003 | 28.774 | 39.138 | 73.895 | 1520 | 1520 | 0 | 0/30 | 0/30 | ok |
| Q143 | unicode-off | 2883.606 | 3087.768 | 3604.408 | 3649.133 | 2057 | 2251 | 2251 | 0/30 | 0/30 | ok |
| Q167 | unicode-off | 3312.947 | 3541.859 | 4848.924 | 5118.836 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | unicode-off | 2913.074 | 3099.475 | 3613.022 | 4625.351 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | unicode-off | 2693.127 | 2939.768 | 4237.420 | 4541.360 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q194 | unicode-off | 161.507 | 195.366 | 266.032 | 295.694 | 7 | 203 | 203 | 0/30 | 0/30 | ok |
| Q202 | unicode-off | 87664.578 | 91862.262 | 99807.503 | 99841.713 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: MemoryOnly, residency: FullyResident } open/build 1.274 s; parses 148486; resident 616 MiB; peak resident 697 MiB; managed files 0 MiB.
- "unicode": IndexStoragePolicy { location: MemoryOnly, residency: FullyResident } open/build 1.247 s; parses 148486; resident 1087 MiB; peak resident 1151 MiB; managed files 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
