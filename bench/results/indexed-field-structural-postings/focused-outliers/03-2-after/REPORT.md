# Musenalm query checkpoint

Code revision: `491d4b4970d891619c7b5c4d14edf253858279ce`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3672.695 | 4082.137 | 7406.110 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7822.298 | 7944.389 | 10041.333 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 34343.454 | 38689.208 | 87794.119 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 6037.575 | 7234.665 | 13351.652 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 8856.066 | 10041.089 | 11539.992 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 39706.895 | 40668.890 | 90204.632 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.978 s; snapshot write 0.474 s; parses 148486; resident 578 MiB; peak resident 743 MiB; estimated index 67 MiB; snapshot 171 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.033 s; snapshot write 0.448 s; parses 148486; resident 1092 MiB; peak resident 1222 MiB; estimated index 70 MiB; snapshot 173 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
