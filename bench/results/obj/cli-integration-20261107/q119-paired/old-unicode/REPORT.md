# Musenalm query checkpoint

Code revision: `4a93382f0404ae4cc6d1e7c028b7a48cacd006f4`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | unicode-off | 7805.654 | 12368.909 | 13269.228 | 133948 | 133948 | 0 | 19/20 | 0/20 | ok |

## Index construction

- "unicode": build 1.248 s; snapshot write 0.257 s; parses 148486; resident 489 MiB; peak resident 582 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.521/0.534/0.558 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
