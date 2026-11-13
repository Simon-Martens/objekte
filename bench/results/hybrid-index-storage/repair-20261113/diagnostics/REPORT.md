# Hybrid storage repair diagnostics

These are intermediate, profile-off measurements on the configured Musenalm
fixture (148,486 records, ASCII construction, 32 workers). They are not the
seven-pair/full-220-case acceptance campaign. The original diagnostic archive
under `diagnostic-20261113/` is unchanged.

## Construction after owned-buffer transfer

`repair-build-paired-d0ed/` contains three alternating-order, fresh-process
pairs, the raw `sample.json` files, commands, stdout/stderr, and executable
SHA-256 hashes in `trials.json`. Baseline binary revision:
`da8fde68472b8646a63a05b3b3654c58de877301`; candidate binary revision:
`d0ed131399`. Both used `--incremental --alphabet ascii --workers 32`.
Medians are the middle observation of three trials. MiB uses 1,048,576 bytes.

| Metric | Baseline | Candidate | Candidate/baseline |
| --- | ---: | ---: | ---: |
| Complete build | 1,018.16 ms | 1,235.70 ms | 1.214 |
| First batch | 340.65 ms | 340.02 ms | 0.998 |
| Retained RSS | 533.30 MiB | 611.93 MiB | 1.147 |
| Peak RSS | 542.57 MiB | 621.45 MiB | 1.145 |
| RSS after releasing index | 439.12 MiB | 439.07 MiB | 1.000 |

The owned Vec transfer cut the earlier candidate peak from about 702 MiB to
621 MiB, but complete construction and both memory gates still fail. The
similar post-release RSS suggests that the excess is live index ownership,
not a process lifetime leak. This is an inference from RSS, not a complete
allocator accounting.

## Matched query tails before owned-buffer transfer

`repair-tail-matched-e56f/` contains 30 repetitions for each of 9 preselected
cases in ASCII and Unicode, in baseline incremental, memory, fully resident,
and warm on-demand modes. Baseline binary revision is `da8fde6847`; candidate
binary revision is `e56f6654e9`. The runner, commands, hashes, fixture/schema
fingerprints, percentile method, and comparisons are in `comparison.json`;
raw samples, digests, and manifests are retained by mode. Verified-result
cache was disabled and recorded zero hits. Every mode matched baseline result
digests, with no failed samples. Prepared-plan hits are reported separately.

| Candidate mode | Geometric mean of per-case medians | Geometric mean of p95 ratios |
| --- | ---: | ---: |
| Memory | 1.153 | 1.194 |
| Fully resident | 1.275 | 1.518 |
| Warm on-demand | 1.365 | 1.531 |

The median definition is the mean of the middle two ordered nanosecond samples,
rounded half up. p95 is nearest rank (`ceil(0.95 × 30)`). Very short Q008
samples are about 4–11 μs; their high ratios need absolute-time context.
Q167/Q175/Q177 still have material millisecond regressions. These selected
results fail the query limits; they do not substitute for the full campaign.
