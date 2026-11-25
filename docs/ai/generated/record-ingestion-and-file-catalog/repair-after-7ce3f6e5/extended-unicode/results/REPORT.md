# Musenalm query checkpoint

Code revision: `7ce3f6e525fee77f46c71ad7c02a94c7d06a21b7`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 147.951 | 150.584 | 254.535 | 254.535 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5489.847 | 5883.019 | 7119.016 | 7119.016 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 30664.767 | 30963.504 | 33513.819 | 33513.819 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 45.247 | 47.197 | 133.534 | 133.534 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.311 | 28.545 | 41.030 | 41.030 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 159.464 | 163.790 | 199.186 | 199.186 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.831 s; parses 148486; resident 682 MiB; peak resident 730 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
