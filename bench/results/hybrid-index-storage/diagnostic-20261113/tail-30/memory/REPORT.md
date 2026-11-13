# Musenalm query checkpoint

Code revision: `389375b06d06e96229df60eb064b21dc81ac3fba`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 5659.538 | 7246.276 | 11134.139 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 6285.866 | 6995.662 | 10655.855 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 5977.818 | 6591.242 | 9006.231 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 5405.617 | 6147.931 | 9574.260 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 95282.996 | 106703.403 | 116643.342 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: MemoryOnly, residency: FullyResident } open/build 1.767 s; parses 148486; resident 652 MiB; peak resident 709 MiB; managed files 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
