#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
    printf 'usage: %s CHECKOUT OUTPUT_JSONL local|related\n' "$0" >&2
    exit 2
fi

checkout=$(realpath "$1")
output=$(realpath -m "$2")
mode=$3
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
target_dir=${Q4_TARGET_DIR:-$checkout/target/q4-computed-sorting}
profile=${Q4_PROFILE:-release}
if [[ "$profile" != release && "$profile" != dev ]]; then
    printf 'Q4_PROFILE must be release or dev\n' >&2
    exit 2
fi
binary_dir=release
if [[ "$profile" == dev ]]; then
    binary_dir=debug
fi
binary="$target_dir/$binary_dir/q4-computed-sort-bench"
sha=$(git -C "$checkout" rev-parse HEAD)
if [[ ! -x "$binary" || ! -f "$target_dir/built_sha" || $(cat "$target_dir/built_sha") != "$sha" ]]; then
    printf 'build the harness for this checkout with build.sh first\n' >&2
    exit 2
fi
main_hash=$(sha256sum "$script_dir/main.rs" | cut -d ' ' -f 1)
if [[ ! -f "$target_dir/built_main_sha256" || $(cat "$target_dir/built_main_sha256") != "$main_hash" ]]; then
    printf 'harness source changed; run build.sh again\n' >&2
    exit 2
fi

export Q4_SHA=$sha
export Q4_RUSTC
Q4_RUSTC=$(rustc --version)
export Q4_PROFILE=$profile
export Q4_FIXTURE_PARENT="$target_dir/fixtures"
mkdir -p -- "$Q4_FIXTURE_PARENT" "$(dirname -- "$output")"
temporary_output="$output.tmp.$$"
trap 'rm -f -- "$temporary_output"' EXIT
"$binary" "$mode" > "$temporary_output"
mv -- "$temporary_output" "$output"
