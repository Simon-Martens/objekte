# Musenalm query checkpoint

Code revision: `389375b06d06e96229df60eb064b21dc81ac3fba`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 8053.286 | 10813.341 | 16661.146 | 3269 | 4872 | 4872 | 29/30 | 0/30 | ok |
| Q167 | ascii-off | 5487.988 | 5948.839 | 8436.196 | 95153 | 95153 | 0 | 29/30 | 0/30 | ok |
| Q175 | ascii-off | 4527.907 | 4641.255 | 4863.664 | 95153 | 95153 | 0 | 29/30 | 0/30 | ok |
| Q177 | ascii-off | 4817.810 | 5305.211 | 7766.440 | 86564 | 86564 | 0 | 29/30 | 0/30 | ok |
| Q202 | ascii-off | 144107.879 | 154988.081 | 177051.838 | 133948 | 133948 | 133948 | 29/30 | 0/30 | ok |

## Index construction

- "ascii": build 1.039 s; snapshot write 0.218 s; parses 148486; resident 473 MiB; peak resident 497 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.599/0.602/0.608 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
