# Musenalm query checkpoint

Code revision: `3daa33e381840636cd2b33169cc746f6714ef343`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 3925.559 | 5375.809 | 6357.661 | 6357.661 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4539.112 | 4584.254 | 5679.803 | 5679.803 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2258.397 | 2348.481 | 2517.356 | 2517.356 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 20.025 | 20.535 | 26.340 | 26.340 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 16.038 | 16.616 | 23.208 | 23.208 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 15.756 | 16.714 | 19.662 | 19.662 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.829 s; parses 148486; resident 664 MiB; peak resident 706 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
