# Musenalm query checkpoint

Code revision: `7c8ce508d54687afc5eeced93611b5662266b74a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3594.438 | 4093.390 | 7410.795 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7680.588 | 8106.875 | 10187.358 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 34515.013 | 37869.807 | 83511.600 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 5425.188 | 5897.704 | 12203.428 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 7841.051 | 8206.647 | 10896.344 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 34457.470 | 38257.379 | 85868.512 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.881 s; snapshot write 0.462 s; parses 148486; resident 584 MiB; peak resident 748 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.070 s; snapshot write 0.447 s; parses 148486; resident 1103 MiB; peak resident 1232 MiB; estimated index 69 MiB; snapshot 172 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
