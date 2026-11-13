# Musenalm query checkpoint

Code revision: `389375b06d06e96229df60eb064b21dc81ac3fba`. Repetitions: 30. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 6917.458 | 8878.867 | 12434.814 | 3269 | 4872 | 4872 | 0/30 | 0/30 | ok |
| Q167 | ascii-off | 6692.122 | 7354.661 | 13507.074 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q175 | ascii-off | 6197.533 | 7056.621 | 10099.343 | 95153 | 0 | 0 | 0/30 | 0/30 | ok |
| Q177 | ascii-off | 5903.229 | 6356.293 | 8668.637 | 86564 | 0 | 0 | 0/30 | 0/30 | ok |
| Q202 | ascii-off | 118948.945 | 127188.944 | 142932.076 | 133948 | 133948 | 133948 | 0/30 | 0/30 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("/home/simon/source/objekte-integration-hybrid-index-storage/target/hybrid-index-storage/stores-smoke"), residency: OnDemand } open/build 0.415 s; parses 0; resident 264 MiB; peak resident 316 MiB; managed files 122 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
