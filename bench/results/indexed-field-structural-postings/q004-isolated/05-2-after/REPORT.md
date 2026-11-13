# Musenalm query checkpoint

Code revision: `491d4b4970d891619c7b5c4d14edf253858279ce`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q004 | unicode-off | 7591.211 | 8448.662 | 22809.301 | 2995 | 7773 | 7773 | 0/15 | 0/15 | ok |

## Index construction

- "unicode": build 1.064 s; snapshot write 0.486 s; parses 148486; resident 605 MiB; peak resident 772 MiB; estimated index 70 MiB; snapshot 173 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
