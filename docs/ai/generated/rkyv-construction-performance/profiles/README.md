# Detailed profiling run

`ascii-32-detailed.json` was produced with:

```text
cargo build -p objekte-search-core --release --locked \
  --features detailed-profiling \
  --example bench_build_musenalm \
  --target-dir target-followup-profile

target-followup-profile/release/examples/bench_build_musenalm \
  --root examples/musenalm \
  --alphabet ascii \
  --build-workers 32 \
  --profile
```

This is a separately instrumented profile and is not part of the uninstrumented
construction comparison.
