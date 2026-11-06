# Controlled Musenalm subset

Before: `084623a19b`. After: `92cdb92b19`. Eleven timed repetitions; each core ran alone in its own process with 32 query workers, disabled verified/source/decoded caches and both alphabets. The same physical corpus and schema files were used.

Resolved query JSON, corpus fingerprint, schema fingerprint, compiler, worker count, and return modes agree. The workload path hash differs because the old source was compiled from a sparse worktree; resolved queries are byte-equivalent.

| Case | Alphabet | Before ms | After ms | Δ ms | Δ% |
|---|---|---:|---:|---:|---:|
| Q006 | ascii-off | 227.346 | 244.480 | +17.134 | +7.5% |
| Q006 | unicode-off | 228.256 | 242.509 | +14.253 | +6.2% |
| Q007 | ascii-off | 286.124 | 280.346 | -5.778 | -2.0% |
| Q007 | unicode-off | 269.593 | 290.803 | +21.210 | +7.9% |
| Q014 | ascii-off | 17.907 | 17.347 | -0.560 | -3.1% |
| Q014 | unicode-off | 10.463 | 10.393 | -0.070 | -0.7% |
| Q099 | ascii-off | 8.300 | 2.573 | -5.727 | -69.0% |
| Q099 | unicode-off | 8.482 | 10.066 | +1.584 | +18.7% |
| Q100 | ascii-off | 16.799 | 16.284 | -0.515 | -3.1% |
| Q100 | unicode-off | 13.854 | 14.850 | +0.996 | +7.2% |
| Q148 | ascii-off | 4.950 | 8.065 | +3.114 | +62.9% |
| Q148 | unicode-off | 6.795 | 5.826 | -0.970 | -14.3% |

These focused timings supplement the fixed 150-query protocol. They do not replace its order-dependent results.
