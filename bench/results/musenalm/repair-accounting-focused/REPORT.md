# File-accounting focused comparison

A is the seek-only binary built from `c5787c1d80` before the accounting edit;
B is the same branch with the accounting edit. The latter was measured in a
dirty working tree; the accompanying code commit identifies it. Both release
binaries ran against the same Musenalm corpus, compiler, 32 workers, ASCII,
verified cache disabled, zero source/decoded cache, OS-warm files, affinity
`0-31`, and `powersave` governor. Process order was A-B-B-A with 11 timed
repetitions per case in each block. All raw samples, commands, counters,
plans and console logs are retained in the block directories.

| Query | A block medians ms | B block medians ms | Pooled A median ms | Pooled B median ms | Difference ms |
|---|---:|---:|---:|---:|---:|
| Q006 | 248.395, 237.987 | 235.734, 265.829 | 241.567 | 239.515 | -2.052 |
| Q025 | 119.433, 129.713 | 93.982, 126.950 | 122.841 | 124.105 | +1.264 |
| Q116 | 122.131, 123.247 | 116.311, 145.640 | 122.689 | 128.082 | +5.393 |
| Q099 | 8.622, 10.945 | 9.588, 4.586 | 9.998 | 6.160 | -3.838 |

The block variation exceeds the apparent accounting effect, so latency
attribution remains unresolved. This does not establish a material regression
or improvement. The structural change removes the separate serial hash-set
insertion pass before broad source scans: a single collection with locator
output counts already unique grouped files during outcome collection, while
overlapping collections and payload reads use a compact integer bitmap.
Tests prove that opens and distinct physical files remain different counters
across shared collections, direct payloads, verified-cache payloads, reader
eviction and index-only locator results.
