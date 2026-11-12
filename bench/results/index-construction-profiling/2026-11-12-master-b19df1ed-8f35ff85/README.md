# Index construction profiling evidence

Task: `index-construction-profiling`, 12 November 2026. The starting master
revision was `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`. The paired full
corpus measurements used `98b070711d2e40631b3a7afc7665404042610fa7`;
the configured subset generator was extended at `3044443ef9a2`; the later
`/proc/self/io` measurements used `8f35ff8510f1`. The core construction path
is identical in those last three revisions.

The tree preserves `sample.json` and `run-manifest.json` for each trial. Paths
inside the original raw JSON refer to `target/index-construction-profiling/`;
replace that prefix with this directory to locate the archived copy. `paired-final`
contains seven pairs per alphabet after warm-ups; `diagnostics-final` contains
the bounded matrix; `io-final` contains process I/O deltas. `reference-*` holds
the separate Hyperfine baseline. `semantic-*` contains the query comparisons.

`gprof-*`, `gmon.*.gz`, and `gmon-unicode.*.gz` are CPU sampling evidence from a
separate `-pg` release build. `callgrind.*` and `dhat.*` are Valgrind evidence
from the configured 5,338-record subset. `system-*` contains `/usr/bin/time -v`
observations. `workspace-tests-delivery.log.gz` and `clippy-delivery.log.gz`
are the final checks. Raw tool text and logs are gzip-compressed without
changing their contents. `analysis.txt` is a derived tabulation; the JSON
samples are the source of all report medians. `samply-capability.stderr.gz`
records why Linux perf sampling was unavailable.

The two generated fixtures and six 129–132 MB snapshot outputs are omitted.
Their manifests and snapshot timing/size counters are retained. Fixtures can
be regenerated with `bench/generate_construction_profile_fixture.py`. Tool
binaries and build intermediates are also omitted. This archive contains no
source records from Musenalm.
