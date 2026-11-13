#!/usr/bin/env bash
set -euo pipefail

# Run from the integration worktree after building the release benchmark and
# creating the one-time restore fixture. Each invocation is a fresh process.
root=/home/simon/source/objekte/examples/musenalm
results=bench/results/index-creation-loading-profiling
benchmark=target/release/examples/bench_index_construction
snapshot=$(find "$results/restore-fixture-final" -name index.snapshot -type f -print -quit)
test -n "$snapshot"

for round in $(seq -w 1 7); do
  if (( 10#$round % 2 == 1 )); then
    construction=(off on)
    restore=(on off)
  else
    construction=(on off)
    restore=(off on)
  fi
  for mode in "${construction[@]}"; do
    profile=()
    if [[ $mode == on ]]; then profile=(--profile); fi
    "$benchmark" --root "$root" --incremental --alphabet ascii --workers 32 \
      "${profile[@]}" --check-queries \
      --result-dir "$results/construction-$mode-$round"
  done
  for mode in "${restore[@]}"; do
    profile=()
    if [[ $mode == on ]]; then profile=(--profile); fi
    "$benchmark" --root "$root" --alphabet ascii --workers 32 \
      "${profile[@]}" --restore-snapshot "$snapshot" \
      --result-dir "$results/restore-$mode-$round"
  done
done
