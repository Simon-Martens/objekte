# Musenalm query checkpoint

Code revision: `389375b06d06e96229df60eb064b21dc81ac3fba`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 13531.975 | 13531.975 | 13531.975 | 3269 | 4872 | 4872 | 0/1 | 0/1 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("/home/simon/source/objekte-integration-hybrid-index-storage/target/hybrid-index-storage/stores-smoke"), residency: FullyResident } open/build 0.508 s; parses 0; resident 316 MiB; peak resident 369 MiB; managed files 122 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
