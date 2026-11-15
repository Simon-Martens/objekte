# Sampling profile unavailable

The representative profile command was:

```bash
perf record -F 99 -g -o perf.data -- \
  target/release/examples/bench_rocksdb_index_store \
  --mode source-build --root /home/simon/source/objekte/examples/musenalm \
  --schema-profile basic --alphabet ascii --storage memory \
  --prepare-workers 8 --write-workers 1 --result-dir <fresh-task-directory>
```

`perf` failed before launching the workload because the host reports
`kernel.perf_event_paranoid=2` and SELinux denied CPU PMU access. No profile or
flamegraph was produced. The report uses stage timers, operation counts and
process user/system accounting instead and does not present summed worker time
as CPU time.
