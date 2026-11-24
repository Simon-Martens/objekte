# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 147.614 | 150.443 | 272.121 | 272.121 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5605.873 | 6004.610 | 7450.269 | 7450.269 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 31616.824 | 32081.100 | 34355.197 | 34355.197 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 44.870 | 46.823 | 138.679 | 138.679 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.021 | 28.385 | 40.522 | 40.522 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 157.125 | 159.288 | 194.098 | 194.098 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.784 s; parses 148486; resident 660 MiB; peak resident 707 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
