# Musenalm query checkpoint

Code revision: `491d4b4970d891619c7b5c4d14edf253858279ce`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q004 | unicode-off | 7795.938 | 9936.836 | 21128.409 | 2995 | 7773 | 7773 | 0/15 | 0/15 | ok |

## Index construction

- "unicode": build 0.984 s; snapshot write 0.463 s; parses 148486; resident 601 MiB; peak resident 768 MiB; estimated index 70 MiB; snapshot 173 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
