# Musenalm query checkpoint

Code revision: `7c8ce508d54687afc5eeced93611b5662266b74a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 4032.560 | 4693.520 | 6364.359 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7597.632 | 8755.512 | 9416.583 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 40286.929 | 42694.672 | 90291.431 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 4877.008 | 5900.531 | 7922.736 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 7637.905 | 8125.631 | 8832.361 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 35123.779 | 38446.133 | 94569.363 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.979 s; snapshot write 0.462 s; parses 148486; resident 586 MiB; peak resident 750 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.047 s; snapshot write 0.456 s; parses 148486; resident 1099 MiB; peak resident 1229 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
