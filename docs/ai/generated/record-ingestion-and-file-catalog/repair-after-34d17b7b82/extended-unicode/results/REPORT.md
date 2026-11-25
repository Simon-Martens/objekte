# Musenalm query checkpoint

Code revision: `34d17b7b82c8398fa906f3c64e611e36fec7c521`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 146.562 | 150.288 | 258.988 | 258.988 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5985.526 | 6116.935 | 7323.052 | 7323.052 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 31062.275 | 32698.719 | 33848.066 | 33848.066 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 45.776 | 48.831 | 134.501 | 134.501 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.079 | 28.332 | 40.820 | 40.820 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 158.123 | 163.142 | 199.301 | 199.301 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.798 s; parses 148486; resident 671 MiB; peak resident 713 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
