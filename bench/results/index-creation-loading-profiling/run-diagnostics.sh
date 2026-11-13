#!/usr/bin/env bash
set -euo pipefail

root=/home/simon/source/objekte/examples/musenalm
results=bench/results/index-creation-loading-profiling
benchmark=target/release/examples/bench_index_construction

# Sequential, fresh-process worker scaling with the configured ASCII baseline.
for workers in 1 8 32; do
  for round in 1 2 3; do
    "$benchmark" --root "$root" --incremental --alphabet ascii \
      --workers "$workers" \
      --result-dir "$results/workers-$workers-$round"
  done
done

# Separate diagnostic configurations. They are not pooled with the baseline.
for round in 1 2 3; do
  "$benchmark" --root "$root" --incremental --alphabet unicode \
    --workers 32 --profile \
    --result-dir "$results/unicode-$round"
done
"$benchmark" --root "$root" --incremental --alphabet ascii \
  --workers 32 --plain --profile \
  --result-dir "$results/plain-1"
"$benchmark" --root "$root" --incremental --alphabet ascii \
  --workers 32 --inference --profile \
  --result-dir "$results/inference-1"
