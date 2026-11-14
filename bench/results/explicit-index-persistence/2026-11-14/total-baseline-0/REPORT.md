# Musenalm query checkpoint

Code revision: `b54822f9afdf844e90b53aa73e1374c58983dcf0`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 11552.594 | 11552.594 | 11552.594 | 11552.594 | 3269 | 4872 | 4872 | 0/1 | 0/1 | ok |

## Index construction

- "ascii": IndexStoragePolicy { location: CustomDirectory("/home/simon/source/objekte-integration-explicit-index-persistence/target/explicit-index-persistence/paired-total-stores/baseline-0"), residency: OnDemand } open/build 1.807 s; parses 148486; resident 606 MiB; peak resident 720 MiB; managed files 135 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
