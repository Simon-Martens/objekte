# Musenalm query checkpoint

Code revision: `491d4b4970d891619c7b5c4d14edf253858279ce`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3801.360 | 5411.545 | 10018.004 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7752.474 | 8187.588 | 9163.969 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 32050.986 | 36972.304 | 93339.115 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3851.194 | 4943.557 | 7857.078 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 7986.573 | 8488.186 | 10011.019 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 30119.734 | 34865.189 | 85352.824 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.960 s; snapshot write 0.462 s; parses 148486; resident 598 MiB; peak resident 763 MiB; estimated index 67 MiB; snapshot 171 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.078 s; snapshot write 0.462 s; parses 148486; resident 1109 MiB; peak resident 1239 MiB; estimated index 70 MiB; snapshot 173 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
