# Musenalm query checkpoint

Code revision: `7c8ce508d54687afc5eeced93611b5662266b74a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 4160.464 | 5416.238 | 6960.395 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7783.479 | 8441.132 | 8989.428 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 35216.817 | 37363.266 | 87668.442 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 4366.919 | 6103.442 | 12132.243 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 7818.085 | 8101.894 | 10976.930 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 37339.335 | 40523.204 | 82135.954 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.927 s; snapshot write 0.461 s; parses 148486; resident 583 MiB; peak resident 748 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.077 s; snapshot write 0.451 s; parses 148486; resident 1101 MiB; peak resident 1231 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
