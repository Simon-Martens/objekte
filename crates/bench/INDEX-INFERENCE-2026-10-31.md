# Observed field inference build cost, 2026-10-31

Release build of `index-build-infer` against all seven collections in `examples/musenalm`. Each sample used a fresh process and process-local index; five runs per mode alternated. Index timing instrumentation was disabled. The table shows median wall time for `warm_bigram_index` in milliseconds. All source files were indexed, with no query limit.

| Collection | Inference off | Inference on | Increase |
| --- | ---: | ---: | ---: |
| agents | 133.2 | 234.7 | 76.2% |
| contents | 1,296.8 | 1,771.3 | 36.6% |
| entries | 172.0 | 232.9 | 35.4% |
| items | 19.2 | 33.8 | 76.0% |
| pages | 0.5 | 0.6 | 20.0% |
| places | 1.6 | 2.1 | 31.2% |
| series | 8.6 | 14.1 | 64.0% |
| **All collections** | **1,631.2** | **2,287.9** | **40.3%** |

Separate `/usr/bin/time` runs reported peak resident memory of **261,164 KiB** without inference and **342,080 KiB** with inference, an increase of **80,916 KiB** (31.0%). The process also retains inferred postings after building; this peak is not a precise steady-state index size. Inference is opt in with `--infer-field-types` or `Options::infer_field_types`.

Reproduce:

```sh
cargo build --release -p objekte-bench --bin index-build-infer
target/release/index-build-infer examples/musenalm
target/release/index-build-infer examples/musenalm --infer
/usr/bin/time -f 'peak_kb=%M' target/release/index-build-infer examples/musenalm --infer
```
