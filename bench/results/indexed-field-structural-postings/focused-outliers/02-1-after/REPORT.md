# Musenalm query checkpoint

Code revision: `491d4b4970d891619c7b5c4d14edf253858279ce`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3552.071 | 4053.849 | 4991.904 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | ascii-off | 7703.047 | 8810.676 | 14640.240 | 2995 | 7766 | 7766 | 0/7 | 0/7 | ok |
| Q191 | ascii-off | 30564.121 | 35316.933 | 79437.916 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |
| Q003 | unicode-off | 3601.803 | 4156.581 | 5402.893 | 2866 | 2948 | 2948 | 0/7 | 0/7 | ok |
| Q004 | unicode-off | 7987.647 | 9555.557 | 21834.729 | 2995 | 7773 | 7773 | 0/7 | 0/7 | ok |
| Q191 | unicode-off | 34835.277 | 36497.499 | 56925.269 | 95153 | 0 | 0 | 0/7 | 0/7 | ok |

## Index construction

- "ascii": build 0.967 s; snapshot write 0.486 s; parses 148486; resident 595 MiB; peak resident 759 MiB; estimated index 67 MiB; snapshot 171 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.
- "unicode": build 1.060 s; snapshot write 0.506 s; parses 148486; resident 1106 MiB; peak resident 1236 MiB; estimated index 70 MiB; snapshot 173 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
