# Index construction profile on master

Task: `index-construction-profiling` · measured 12 November 2026 · starting
master `b19df1ed4a38c10c879fa3b0c72b1650a0dab203`.

## Decision summary

On the configured Musenalm corpus, the unchanged construction policy builds
148,486 records in a median **2,519 ms ASCII** or **2,673 ms Unicode** with
profiling disabled. Seven paired warm trials show that opt-in profiling adds a
median 0.06% and 0.54%, respectively. The largest exclusive wall intervals are
source extraction and index admission (1,309–1,472 ms), discovery (652–654 ms),
collection finalization (426–435 ms), and global relation resolution
(111–113 ms). Worker times, CPU samples, and allocator totals overlap these
intervals; they are separate work attribution, not additional wall time.

The next investigation should focus on ordered batch admission, discovery's
metadata/catalogue pass, and serial bigram finalization. The evidence below
sets upper bounds, not promised gains. This task makes no production
optimization or indexing-policy change.

## Reproduction and boundaries

The release benchmark is
`cargo build --release --locked -p objekte-search-core --example bench_index_construction`
followed by `target/release/examples/bench_index_construction --alphabet ascii
--profile --result-dir <directory>` (replace `ascii` with `unicode`, omit
`--profile` for the control). Each invocation opens a fresh process. The
`build_ns` interval starts immediately before `Repository::open_with_options`
and ends when it returns; query checks, snapshot writing, fingerprinting for
the benchmark manifest, and report output occur afterward. `open_ns` includes
the call boundary and differs by only about 0.02–0.03 ms. Source fingerprints
and normal validation remain inside construction.

The normal release binary and paired measurements used instrumentation commit
`98b070711d2e40631b3a7afc7665404042610fa7`; the harness started at
`17b8beaaac5b`. The configured subset generator changed at `3044443ef9a2`.
The later `8f35ff8510f1` change adds benchmark-only `/proc/self/io` counters;
core construction code is identical to `98b0707`. Every trial's exact command,
revision, dirty state, Rust version, worker count, source/schema fingerprints,
and CPU details are in its `run-manifest.json`. Raw samples and tool outputs
are in [the evidence archive](../../../bench/results/index-construction-profiling/2026-11-12-master-b19df1ed-8f35ff85/README.md).

Host: AMD Ryzen 7 5700G, 8 cores/16 threads, 30 GiB RAM, Fedora 44,
Linux 7.2.13, Rust 1.99.1, `powersave` governor, 16 resolved build workers.
The real `musenalm_extended` workload has 148,486 JSON files/records,
193,297,230 source bytes, source inventory fingerprint
`db869a76f26e1a3275a5162e5a207f47fb50fa8be02c4bdcfe3f1db50cfb747c`,
schema fingerprint `4115070eb30b0af98b109429d0883b493f857cdf671ec55b943d91b73c5ebfe3`.
It produces 300 configured posting fields, eight sort fields, 228,503 relation
edges, 15 bigram stores and 25,477 distinct ASCII gram keys, with zero failed
parses. The different inventory fingerprint in the archived staged-branch
benchmark reflects this checkout's file metadata; its phase numbers are not
mixed into this report.

All runs are warm-cache. The seven main pairs per alphabet alternate control
and profiled order after warm-up for each mode. Quartiles use the inclusive
sample method. No query or snapshot is part of these timings.

| Alphabet | Control median [IQR], ms | Profiled median [IQR], ms | Paired median overhead | Paired overhead range |
| --- | ---: | ---: | ---: | ---: |
| ASCII | 2,519.428 [31.179] | 2,509.750 [24.736] | +0.062% | −2.059% to +0.838% |
| Unicode | 2,672.899 [55.333] | 2,691.567 [30.453] | +0.535% | −1.872% to +1.708% |

The profiled ASCII median being slightly faster than its control median is
run-to-run variation, not a speedup. Both overheads are below the task's ~3%
acceptance threshold. Hyperfine's separate unprofiled process-wall baseline
after one warm-up was 3.247 s ASCII and 3.375 s Unicode (seven trials); that
includes startup and post-build benchmark output and is not the `build_ns`
metric. Its raw JSON is archived.

## Exclusive elapsed timeline

Each row below is from the **single trial whose profiled build equals the
seven-run median** for its alphabet. These rows sum exactly. The discovery,
extraction, and finalization sub-tables that follow use per-field medians and
therefore need not sum to a median total.

| Top-level exclusive interval | ASCII ms | Unicode ms | Boundary |
| --- | ---: | ---: | --- |
| Discovery | 647.479 | 653.539 | enumerate files and build inventory/catalogue |
| Build pool initialization | 0.723 | 0.550 | Rayon pool creation |
| Extraction and index construction | 1,300.916 | 1,472.894 | prepare builders, execute/admit jobs, assemble fingerprints |
| Per-collection finalization | 429.795 | 432.513 | finish generations, bigrams, validation, sorting, inference, identities |
| Global relation resolution | 113.340 | 112.701 | resolve and validate relations after collections |
| Final checks/status | 1.030 | 1.111 | remaining checks/status refresh |
| Pool teardown | 0.062 | 0.114 | release build pool |
| Query pool initialization | 0.622 | 0.641 | repository setup before return |
| Unattributed top-level remainder | 15.781 | 17.504 | other setup, bookkeeping and timer boundaries |
| **Build total** | **2,509.750** | **2,691.567** | `build_ns` |

For the seven profiled runs, discovery's median components are enumeration
**97.763/96.884 ms** and metadata, path handling, catalogue/inventory insertion
**554.224/557.135 ms** (ASCII/Unicode). The extraction interval contains
builder/setup **3.779/3.984 ms**, source work including worker execution and
ordered admission **1,292.701/1,455.073 ms**, and final fingerprint assembly
**12.812/12.657 ms**; its median within-trial remainder is only
**0.066/0.066 ms**. Source hashing during reads is counted in worker source
work, not in final fingerprint assembly.

| Collection finalization component, median ms | ASCII | Unicode |
| --- | ---: | ---: |
| Generation | 39.987 | 38.856 |
| Bigram index assembly/compression | 195.263 | 209.243 |
| Validation | 8.484 | 8.849 |
| Sort index completion | 151.162 | 149.643 |
| Inference finish (disabled here) | 0.001 | 0.001 |
| Identity order/lookups | 29.258 | 28.932 |
| Within-finalization remainder | 0.022 | 0.026 |

The finalization subcomponents are sequential, but medians of individual
components are not additive. Global relation resolution is outside this table.

## Worker, source, and coordinator work

The regular file path produced a median **294 batches**. Across 16 workers,
the summed job elapsed durations were **8,161/10,518 ms** for ASCII/Unicode.
Those durations overlap each other and the coordinator; they are not CPU time
or an exclusive part of the 1,293/1,455 ms source-work wall interval. The
coordinator spent **144/203 ms receiving/waiting** and **1,149/1,237 ms
admitting batches** in order. The measured admission interval includes
remapping and merging contributions, but overlaps worker execution. It cannot
be added to source-work wall time.

At least **114/131 batches** (median) completed out of order; peak ready queue
was **31/31** and maximum wait by one completed batch was **1,283/1,446 ms**.
The sum of ready-batch wait durations, **6,585/8,113 ms**, also overlaps across
batches. This confirms head-of-line waiting, but the maximum wait is not an
estimate of potential wall savings. Peak admitted weight was 255.23 MB against
a 268.44 MB limit; peak active jobs reached 16. The measurements do not record
worker idle time from weight backpressure, so its elapsed impact remains
unknown.

The record-level opt-in counters sample every 64th ordinary file/record. In
the median ASCII run they cover **2,321 files/records, 3,060,949 bytes and
17,804 reader calls**. The following are **summed sampled worker durations**;
they overlap parallel work and should not be scaled into wall fractions. A
reader call is a Rust `Read` invocation, not necessarily an OS read syscall.

| Sampled operation | ASCII ms | Unicode ms |
| --- | ---: | ---: |
| Open | 9.337 | 9.482 |
| Read calls, including memory copying | 16.348 | 16.607 |
| Hash while reading | 1.672 | 1.697 |
| Framing/record boundary | 0.129 | 0.131 |
| Source extraction, **inclusive** of callback | 103.143 | 132.924 |
| Callback within source extraction | 71.111 | 102.547 |
| Declared/schema validation | 6.163 | 6.060 |
| Structured postings | 19.100 | 18.886 |
| Inference observation (disabled) | 0.079 | 0.086 |
| Relation contributions | 0.960 | 1.020 |
| Custom postings | 0.084 | 0.082 |
| Text traversal, normalization and bigram insertion | 33.374 | 65.118 |
| Sort key extraction/collation | 5.148 | 5.409 |
| Custom text | 1.017 | 0.248 |
| Batch finish across 294 batches | 2.347 | 2.279 |

Parsing and logical-value construction are fused in source extraction; their
individual wall time is not separately timed. Source extraction includes
callback work, so the callback must not be added to it. The Unicode sampled
text/bigram stage is roughly twice ASCII's, consistent with the full
source-work interval growing by 162 ms. No decoded-text byte counter is
available, so source bytes are the only measured byte denominator for the
full corpus.

Warm `/proc/self/io` deltas around `Repository::open_with_options` were
**193,302,128/193,302,104 rchar bytes** and **310,179/310,175 syscr** for
ASCII/Unicode (three-run medians). The extra ~4.9 KB above source bytes is
ancillary input. `read_bytes` and `/usr/bin/time` filesystem inputs were zero:
the corpus was served from cache. These counters do not measure a cold disk
path. The sampled read duration includes CPU work and is not pure I/O wait.

## CPU, allocation, and memory

A separate full-corpus release build with `-C link-arg=-pg -C
force-frame-pointers=yes` produced gprof CPU samples. ASCII self samples were
led by Unicode case folding **20.62%**, sort unavailable-value observation
**6.04%**, `BigramBuilder::new` **5.15%**, Roaring insertion **4.71%**, scalar
posting B-tree search **2.80%**, bitmap push **2.80%**, UTF-8 conversion
**2.65%**, record-fold guard drop **2.65%**, and SHA-256 blocks **2.36%**.
Unicode's leaders were gram-word setting **15.34%**, case folding **15.34%**,
hashing a gram index **9.27%**, fold-guard drop **5.30%**, and Roaring insertion
**4.86%**. JSON parsing/value conversion, allocation, collation, and bitmap
merges also appear in the archived flat profiles. gprof samples process CPU,
including work outside the build interval, at 10 ms resolution; multi-thread
sampling may be incomplete. These percentages are not percentages of build
wall time.

Callgrind on a **separate configured 5,338-record subset**, with four workers,
recorded 4.182 billion instructions: case folding **25.80%**, SHA-256
compression **16.39%**, `BigramBuilder::add_segment` **11.02%** of exclusive
instructions. It corroborates costly text/hash work but cannot be scaled to
the full 148,486-record wall interval. On the same subset, Valgrind DHAT
recorded **296.87 MB allocated in 1,387,369 blocks**, approximately **55.6 KB
and 260 blocks per record** including setup/teardown; peak live app heap under
DHAT was **25.03 MB**. Largest individual allocation stacks were JSON-to-
logical-value vector construction (20.64 MB), logical-value B-tree assembly
(15.60 MB), serde JSON map insertion (14.37 MB), per-file `read_to_end`
growth (12.17 MB), and logical-value sorting (9.44 MB). These are lifetime
allocations and do not equal peak RSS or prove a corresponding elapsed saving.

For a normal fresh profiled ASCII/Unicode build, median RSS after discovery was
**88.8/88.7 MB**, after extraction **529.1/543.9 MB**, after finalization
**540.6/573.6 MB**, and just after build **529.2/557.7 MB**. Peak RSS reported
by the process was **566.1/589.2 MB**; after repository release RSS remained
**419.5/428.5 MB**. The high post-release RSS likely includes allocator-retained
pages and is not evidence of live repository data. Separate `/usr/bin/time -v`
runs reported maximum RSS **539,048/569,988 KiB**, user time **9.78/11.81 s**,
system time **3.04/2.94 s**, CPU use **386%/432%**, zero major faults, and
**174,713/182,490 minor faults**. Their voluntary context switches were
10,916/10,151; involuntary switches 1,514/1,529. These values vary across
fresh processes and should not be combined as one run.

Inspectable but nonexclusive heap estimates were inventory/path/record-ID
storage **83.68 MB**, relation structures **21.47 MB**, and selected index
payloads **69.66/72.57 MB** ASCII/Unicode. Estimated index bytes combine
component heap estimates and serialized posting sizes; they do not include
all capacities, temporary batches, worker buffers, allocator bookkeeping, or
query caches. The remaining RSS is therefore not an unexplained live-index
slice. Shared Arc-owned data is not counted twice in these estimates. Peak
admitted weight of 255.23 MB is a scheduling weight, not a live-heap measure.

## Diagnostic matrix and persistence

These are separate profiled warm runs, usually three each; the main 16-worker
point has seven. Configured, explicit AutoIndex inference, plain, and JSONL
have different semantics and are not equivalent outputs.

| Scenario | Median build ms [IQR] | Key observation |
| --- | ---: | --- |
| Configured ASCII, 1 worker | 7,671.752 [27.602] | extraction 6,342 ms; bigram finalization 324 ms |
| Configured ASCII, 4 workers | 2,929.824 [2.805] | extraction 1,742 ms |
| Configured ASCII, 16 workers | 2,509.750 [24.736] | extraction 1,309 ms |
| Configured ASCII + AutoIndex | 3,800.409 [10.570] | source work 2,333 ms; inference finish 235 ms |
| Configured Unicode + AutoIndex | 3,848.065 [2.609] | source work 2,383 ms; inference finish 232 ms |
| Plain unconfigured ASCII | 1,740.893 [7.361] | 0 configured postings/sorts/relations; still 2 text stores and mandatory metadata |
| One-file JSONL ASCII | 111.854 [2.903] | 16,384 records, 15,380,970 bytes, 64 chunks |

One to four workers saves 4,742 ms, whereas four to 16 saves only 420 ms.
The remaining serial discovery/finalization/relation intervals, admission,
memory pressure, and scheduling can all limit scaling; this matrix does not
isolate their relative causal shares. Explicit AutoIndex has 300 configured
posting fields, eight sort fields, and 228,503 relation edges like configured
mode, plus inferred state; it adds about **1.29 s** ASCII over the profiled
configured point. Its 235 ms inference finish is only part of that difference.
The plain collection still builds text bigrams, with a 175 ms finalization
component; it is not a base-only index policy.

The JSONL fixture is one generated `records.jsonl`, not the full configured
corpus. Its **19.086 ms framing timer** covers repeated bounded `next_chunk`
calls and source read/hash/framing; 64 chunks cover the 15.38 MB source. The
whole large-file source path is **84.029 ms** (median) and overlaps that
framing timer; it does not expose a separate per-record sample. `/proc/self/io`
shows median **15,385,833 rchar**, **1,918 syscr**, and zero physical
`read_bytes` for this warm fixture. Its bigram finalization is **21.504 ms**.

Snapshot writing was measured **after build** in three separate runs per
alphabet. Median snapshot sizes were **129,139,458 bytes ASCII** and
**131,650,034 bytes Unicode**; total write phases took **364.398/373.755 ms**.
Inside that total, section serialization took **171.109/176.374 ms**,
checksums **57.931/59.255 ms**, writes **54.626/56.808 ms**, inventory
preparation **18.453/18.206 ms**, file sync **48.104/48.277 ms**, and
directory sync **6.070/6.175 ms**. These are nested/sequential phase counters
from the current snapshot contract; no independent compression timer exists.
RSS after snapshot was **565.027/507.802 MB** in median-of-field terms, with
large process-to-process variance, so it does not establish a persistent
snapshot memory increase. Snapshot cost is excluded from every build timing
above.

## Ranked opportunities for a later task

1. **Ordered batch admission and head-of-line delay — highest possible impact,
   medium confidence.** The coordinator performs 1,149 ms of admission while
   workers run, and at least 114 completed batches arrive out of order on the
   ASCII corpus. The ready queue reaches 31; one batch waits up to 1,283 ms.
   Investigate which admission operations are truly serial and whether large
   early batches gate later ones. This could reduce serial work, waiting, or
   temporary memory. Its strict optimistic wall-saving bound is the entire
   **1,293 ms source-work interval** (or **1,149 ms** if only admission work
   disappears); actual savings are likely smaller because worker work
   overlaps. An isolated admission trace/benchmark is needed before changing
   scheduling or publication semantics.
2. **Discovery catalogue/metadata pass — high confidence.** Of 652 ms
   discovery, 554 ms is metadata/path/inventory/catalogue work and 98 ms is
   enumeration. This is an exclusive pre-worker interval and remains about
   650 ms even with one worker. Investigate syscall counts and path/inventory
   allocation while preserving metadata validation and identity behavior.
   Eliminating the entire catalogue subinterval could save at most **554 ms**
   ASCII; any realistic improvement saves a fraction of that.
3. **Bigram finalization — high confidence, narrower scope.** Finishing the
   15 configured text stores costs 195 ms ASCII or 209 ms Unicode after source
   jobs; it is separate from sampled per-record text work. Profile gram-to-
   bitmap conversion and merging for allocation/copy reduction while keeping
   query answers identical. Eliminating this whole exclusive stage could save
   at most **195/209 ms** respectively. The separate 151/150 ms sort completion
   and 112/111 ms relation resolution are next serial candidates; their costs
   must not be added to a promised gain.

The CPU profiles also justify a focused follow-up on Unicode case folding,
SHA-256, JSON value conversion, and gram insertion. Their work largely runs
in parallel; even large CPU or allocation shares do not directly translate to
elapsed savings. No feature-disable ablation or semantic shortcut was used.

## Validation and limitations

`cargo test --workspace --locked` passed (808 passed, 0 failed, 17 ignored
across the workspace groups); `cargo clippy --workspace --all-targets --locked`
passed with existing warnings; the focused build tests passed (19 passed,
2 ignored). `git diff --check` and Python fixture-script compilation passed.
The repository-wide Rust spacing check still reports unrelated preexisting
files such as `crates/cli/src/execution.rs`; touched Rust files were formatted
and checked, and unrelated formatter edits were restored.

The profiled and unprofiled builds had identical 148,486 parses, zero failed
parses, 300 configured posting fields, eight sort fields, 228,503 relation
edges, and 69,662,968 estimated ASCII index bytes. Query digests for Q001,
Q108, Q116, Q120, and Q194 matched. The benchmark checks nonnegative
reconciled intervals and sampled-counter invariants. No measured profiling
regression exceeds the ~3% acceptance threshold. Unicode's larger build time
is the existing alphabet workload cost, not an instrumentation regression.

`samply` could not open CPU perf events because this host has
`perf_event_paranoid=2` and noninteractive sudo cannot lower it; gprof and
Valgrind provide the available attribution. Gprof's multi-thread sampling and
Callgrind's small-fixture instruction counts have the limits noted above.
Warm-cache `read_bytes=0` leaves physical storage latency unmeasured. A
colder-data run was omitted to avoid evicting unrelated cache consumers;
advisory data-page eviction would not reset metadata cache anyway. The
instrumentation measures only selected ordinary records in detail, and no
separate decoded-text-byte or worker-idle counter exists. These are explicit
limits on attribution, not gaps filled by assumed fractions.
