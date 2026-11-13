# Musenalm query checkpoint

Code revision: `389375b06d06e96229df60eb064b21dc81ac3fba`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 7073.181 | 9233.977 | 11472.579 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 6719.702 | 7722.344 | 11091.702 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 6229.299 | 6964.859 | 9205.043 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 5680.454 | 6283.989 | 9280.482 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 120801.540 | 128413.627 | 137750.339 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("/home/simon/source/objekte-integration-hybrid-index-storage/target/hybrid-index-storage/stores-smoke"), residency: FullyResident } open/build 0.476 s; parses 0; resident 316 MiB; peak resident 368 MiB; managed files 122 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
