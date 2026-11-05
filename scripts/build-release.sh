#!/usr/bin/env bash
# Build a portable, stripped Linux x86_64 binary without private build paths.
set -euo pipefail
root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/build --remap-path-prefix=$root=/src/ktop"
cargo build --locked --release --target x86_64-unknown-linux-musl
mkdir -p target/release
cp target/x86_64-unknown-linux-musl/release/ktop target/release/ktop
printf 'Release executable: target/release/ktop\n'
