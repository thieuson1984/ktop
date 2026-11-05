#!/usr/bin/env bash
# Collect notices from the exact vendored dependency and Rust distribution.
# Usage: scripts/collect-licenses.sh VENDOR_DIR OUTPUT_DIR
set -euo pipefail
export LC_ALL=C
umask 022
[[ $# == 2 ]] || { printf 'Usage: %s VENDOR_DIR OUTPUT_DIR\n' "$0" >&2; exit 1; }
vendor=$1
output=$2
check_notice_input() {
    local path=$1 parent=$1
    [[ -f "$path" && ! -L "$path" ]] || { printf 'Missing, nonregular or symlinked license input.\n' >&2; return 1; }
    while [[ "$parent" == */* && "$parent" != / ]]; do
        parent=${parent%/*}
        [[ ! -L "$parent" ]] || { printf 'A license input directory is a symlink.\n' >&2; return 1; }
    done
}
parent=$output
while [[ -n "$parent" && "$parent" != / ]]; do
    [[ ! -L "$parent" ]] || { printf 'A license output directory is a symlink.\n' >&2; exit 1; }
    [[ "$parent" == */* ]] || break
    parent=${parent%/*}
done
shopt -s nullglob
crates=("$vendor"/*)
[[ ${#crates[@]} == 1 && -d "${crates[0]}" && ! -L "${crates[0]}" && "${crates[0]##*/}" == libc-* ]] || { printf 'Expected exactly one vendored libc dependency; audit new dependency licenses before release.\n' >&2; exit 1; }
check_notice_input "${crates[0]}/LICENSE-MIT"
check_notice_input "${crates[0]}/LICENSE-APACHE"
sysroot=$(rustc --print sysroot)
notices="$sysroot/share/doc/rust"
if [[ ! -f "$notices/COPYRIGHT-library.html" || ! -f "$notices/COPYRIGHT.html" ]]; then
    printf 'Release packaging requires a current Rust distribution with complete COPYRIGHT-library.html and COPYRIGHT.html inventories; use the stable toolchain. Older Rust may compile the app but lacks this release-notice format.\n' >&2
    exit 1
fi
check_notice_input "$notices/COPYRIGHT-library.html"
check_notice_input "$notices/COPYRIGHT.html"
for path in libc rust; do
    [[ ! -L "$output/$path" ]] || { printf 'A license output directory is a symlink.\n' >&2; exit 1; }
done
mkdir -p "$output/libc" "$output/rust"
for path in libc/LICENSE-MIT libc/LICENSE-APACHE rust/COPYRIGHT-library.html.gz rust/COPYRIGHT-distribution.html.gz rust/TOOLCHAIN; do
    [[ ! -e "$output/$path" && ! -L "$output/$path" || -f "$output/$path" && ! -L "$output/$path" ]] || { printf 'A license output is nonregular or symlinked.\n' >&2; exit 1; }
done
cp -- "${crates[0]}/LICENSE-MIT" "${crates[0]}/LICENSE-APACHE" "$output/libc/"
# Keep both the standard library's bundled third-party notices and the compiler
# distribution notices: cross-target support libraries may use either inventory.
gzip -n -9 -c -- "$notices/COPYRIGHT-library.html" > "$output/rust/COPYRIGHT-library.html.gz"
gzip -n -9 -c -- "$notices/COPYRIGHT.html" > "$output/rust/COPYRIGHT-distribution.html.gz"
rustc --version > "$output/rust/TOOLCHAIN"
