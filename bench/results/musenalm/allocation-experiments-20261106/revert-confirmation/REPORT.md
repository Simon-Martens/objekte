# Musenalm fresh build after source-buffer revert

Measured code: `8387d1b039620aefce8e48f0dcac432dd5eaab42`. This includes the revert `c5db04767c2ebe6b32f7aa9bf3222abdd62de991`; the affected core source files match experiment 3. The release example was compiled before the single measured invocation. The exact command, rustc version, build flags, CPU, affinity, governor and corpus fingerprints are in `metadata.json`. The raw output, stderr and schema manifest are saved alongside it.

Configuration: `examples/musenalm/`, ASCII, snapshots disabled, automatic build workers resolving to 32. This constructed indexes afresh without restoring or writing a snapshot. `wall_ns` measures repository opening; compilation and semantic-signature generation are outside that interval.

| Metric | This run | Experiment 3 | Difference |
| --- | ---: | ---: | ---: |
| Build wall | 1574.195 ms | 1553.532 ms | +20.663 ms (+1.3%) |
| Build phase | 1574.182 ms | 1553.506 ms | +20.677 ms |
| Discovery | 315.772 ms | 308.812 ms | +6.960 ms |
| Extraction/index | 1124.301 ms | 1110.439 ms | +13.862 ms |
| Finalization | 120.062 ms | 119.898 ms | +0.164 ms |
| Resident | 279.441 MiB | 273.047 MiB | +6.395 MiB |
| Peak RSS | 288.766 MiB | 283.539 MiB | +5.227 MiB |

The run was 115.462 ms (6.8%) below the original three-run reference median of 1689.657 ms. It was 8.477 ms below the experiment 4 sample, but the one-sample differences do not establish a performance effect. RSS is resident memory, not an allocation count. Extraction/index includes more than parsing and is not summed worker CPU time.

The schema digest and canonical registrations matched the reference and all four experiment samples. There were 148,486 records, 148,486 successful parses, zero failed parses and 32 resolved workers. The semantic index signature was unchanged: `521402437efd766e5e964cfe5f70443de57d6ad8b5e6e1e61425b9b6dfaa0688`.
