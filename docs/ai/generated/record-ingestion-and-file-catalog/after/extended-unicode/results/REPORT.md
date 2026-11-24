# Musenalm query checkpoint

Code revision: `55f6bb8ba596f1503d232f86a67a58a233c441b4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 150.328 | 153.161 | 293.586 | 293.586 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5894.105 | 6091.589 | 7225.455 | 7225.455 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 31605.311 | 31859.672 | 34502.054 | 34502.054 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 45.705 | 48.720 | 145.536 | 145.536 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.016 | 28.391 | 40.467 | 40.467 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 157.635 | 162.466 | 239.629 | 239.629 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.813 s; parses 148486; resident 661 MiB; peak resident 705 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
