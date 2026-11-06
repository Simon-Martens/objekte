# Foundation performance repair baseline

Starting branch: `development/local-file-search-core` at
`29720036d896e6c6500df444b61a0807a543d60c`. The active core and benchmark
code is unchanged from `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`;
intervening commits only merged the foundation work and changed TUI startup.
The new guide and other pre-existing untracked documentation are outside this
branch's commits.

The immediate before-state is **both**
[`final-92cdb92b19`](../../bench/results/musenalm/final-92cdb92b19/) and
[`final-repeat-92cdb92b19`](../../bench/results/musenalm/final-repeat-92cdb92b19/).
The first is the declared primary recording; the repeat exposes run variation.
[`final-084623a19b`](../../bench/results/musenalm/final-084623a19b/) is the
separate pre-foundation reference. Existing build, controlled and focused
recordings at `92cdb92b19` supply construction and query diagnostics. A new
complete before-state run would duplicate a compatible recording.

The recorded query protocol is release build with Rust 1.99.1, 150 resolved
queries, ASCII and Unicode indexes, disabled and warm 64 MiB verified-cache
profiles, five timed repetitions, 32 query workers, zero source/decoded-cache
budgets, and count/page/payload work inside each timing. Planning diagnostics
are captured outside timed samples. The fixed corpus has 148,486 records,
fingerprint `9f93a4ba7fb9249c9c4eb04e03ed911435a6b0493f15b087a5447add32456aa9`.
Schema fingerprint is `4941bc3e09e4a6a5837375d0ce0db16f9372b469ee2b2971914d9f9527b0957a`;
workload fingerprint is `ca8fe6bb789c3c8bae531c1662d6fe37c9e188a525d1c06b508a85f3209f09f8`.
The original manifests retain the exact commands, query JSON, output modes,
registration data, samples and machine configuration. Indexed-path registration
order is semantically unordered.

This host currently reports an i9-13900K, 32 logical CPUs, `powersave`
governor and affinity `0-31`. The compiler and hardware match the recordings.
Focused paired measurements will use the unmodified `92cdb92b19` core with
matching build flags, affinity, corpus and cache settings. Before/after process
blocks and raw samples will be retained; profiling is outside timed samples.
The full 150-query protocol runs once at the final code revision. Build and
restore measurements use both alphabets, snapshot-disabled and rebuilt modes.
