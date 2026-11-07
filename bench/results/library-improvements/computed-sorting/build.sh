#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: %s CHECKOUT\n' "$0" >&2
    exit 2
fi

checkout=$(realpath "$1")
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
export CARGO_TARGET_DIR=${Q4_TARGET_DIR:-$checkout/target/q4-computed-sorting}
profile=${Q4_PROFILE:-release}
mkdir -p -- "$CARGO_TARGET_DIR"
scratch=$(mktemp -d "$CARGO_TARGET_DIR/scratch.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
mkdir -p -- "$scratch/src"
cp -- "$script_dir/main.rs" "$scratch/src/main.rs"
sed "s|__CORE_PATH__|$checkout/crates/core|g" \
    "$script_dir/Cargo.toml.template" > "$scratch/Cargo.toml"

profile_args=()
if [[ "$profile" == release ]]; then
    profile_args+=(--release)
elif [[ "$profile" != dev ]]; then
    printf 'Q4_PROFILE must be release or dev\n' >&2
    exit 2
fi
cargo build --quiet "${profile_args[@]}" --manifest-path "$scratch/Cargo.toml"
git -C "$checkout" rev-parse HEAD > "$CARGO_TARGET_DIR/built_sha"
sha256sum "$script_dir/main.rs" | cut -d ' ' -f 1 > "$CARGO_TARGET_DIR/built_main_sha256"
