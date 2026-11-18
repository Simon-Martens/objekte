# Musenalm query checkpoint

Code revision: `59089254ac07c195ad52680aa82685cb93ed8b1f`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2697.295 | 3149.338 | 6412.425 | 6412.425 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4180.839 | 4335.841 | 4416.036 | 4416.036 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 1911.252 | 2015.030 | 2221.596 | 2221.596 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 18.923 | 21.178 | 25.325 | 25.325 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.450 | 19.605 | 25.809 | 25.809 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.543 | 16.801 | 21.320 | 21.320 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.773 s; parses 148486; resident 633 MiB; peak resident 675 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
