# Musenalm query checkpoint

Code revision: `4de52f87d20a574a902e3becf454d3ea57a68eac`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 146.386 | 149.445 | 274.381 | 274.381 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5594.493 | 6052.874 | 7375.073 | 7375.073 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 30783.806 | 31086.985 | 33791.445 | 33791.445 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 46.776 | 51.348 | 142.717 | 142.717 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.065 | 28.456 | 40.858 | 40.858 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 157.911 | 159.911 | 194.120 | 194.120 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.779 s; parses 148486; resident 672 MiB; peak resident 714 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
