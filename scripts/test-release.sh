#!/usr/bin/env bash
# Focused release-contract regression checks for one canonical static release.
set -euo pipefail
export LC_ALL=C
[[ $# -ge 1 && $# -le 2 ]] || { printf 'Usage: %s target/release/ARCH/ktop [x86_64|aarch64|armv7]\n' "$0" >&2; exit 1; }
project=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary=$(realpath -- "$1")
# shellcheck source=release-common.sh
source "$project/scripts/release-common.sh"
arch=$(release_arch "${2:-}")
work=$(mktemp -d "${TMPDIR:-/tmp}/ktop-release-check.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
root=$work

[[ $(release_arch amd64) == x86_64 ]]
[[ $(release_arch arm64) == aarch64 ]]
[[ $(release_arch armv7l) == armv7 ]]
[[ $(release_arch armv8l) == armv7 ]]
[[ $(release_arch armhf) == armv7 ]]
[[ $(release_target x86_64) == x86_64-unknown-linux-musl ]]
[[ $(release_target aarch64) == aarch64-unknown-linux-musl ]]
[[ $(release_target armv7) == armv7-unknown-linux-musleabihf ]]
if release_arch mips > "$work/output" 2>&1; then
    printf 'Unsupported architecture was accepted.\n' >&2
    exit 1
fi
# The runner is a single executable, with all argument boundaries preserved.
printf '#!/bin/sh\nexec "$@"\n' > "$work/runner with spaces"
chmod 755 "$work/runner with spaces"
[[ $(KTOP_RUNNER="$work/runner with spaces" release_run aarch64 /bin/printf '%s' 'spaces; no shell') == 'spaces; no shell' ]]
if KTOP_RUNNER="$work/missing runner" release_run "$arch" "$binary" --version > "$work/output" 2>&1; then
    printf 'Unavailable runner was accepted.\n' >&2
    exit 1
fi

printf '[package]\nname = "ktop"\nversion = "0.3.0" # release\n[dependencies]\nversion = "9.9.9"\n' > "$work/Cargo.toml"
[[ $(release_version) == 0.3.0 ]]
for fixture in \
    $'[package]\nversion = "01.3.0"' \
    $'[package]\nversion = "0.3.0-dev"' \
    $'[package]\nversion = "0.3.0"\nversion = "0.4.0"' \
    $'[workspace.package]\nversion = "0.3.0"' \
    $'[package]\nversion.workspace = true'; do
    printf '%s\n' "$fixture" > "$work/Cargo.toml"
    if release_version > "$work/output" 2>&1; then
        printf 'Invalid release metadata was accepted.\n' >&2
        exit 1
    fi
done
root=$project
version=$(release_version)
release_check_binary "$binary" "$version" static "$arch"
runtime=1
if [[ -z "${KTOP_RUNNER:-}" && "$arch" != "$(release_arch)" ]]; then
    runtime=0
else
    if release_check_binary "$binary" 999999999.0.0 static "$arch" > "$work/output" 2>&1; then
        printf 'Mismatched binary version was accepted.\n' >&2
        exit 1
    fi
    grep -F 'versions do not match' "$work/output" >/dev/null
fi
wrong_arch=x86_64
[[ "$arch" != x86_64 ]] || wrong_arch=aarch64
if release_check_binary "$binary" "$version" static "$wrong_arch" > "$work/output" 2>&1; then
    printf 'Mismatched ELF architecture was accepted.\n' >&2
    exit 1
fi
grep -E 'Expected an? (x86_64|AArch64)' "$work/output" >/dev/null
# /bin/true is an existing harmless dynamic ELF on the build host.
if KTOP_RUNNER= release_check_binary /bin/true "$version" static "$(release_arch)" > "$work/output" 2>&1; then
    printf 'Dynamic binary was accepted as a static release.\n' >&2
    exit 1
fi
grep -F 'Expected a static binary' "$work/output" >/dev/null
if [[ "$runtime" == 1 ]]; then
    printf 'Release metadata, architecture, runner, version and static-binary rejection checks passed.\n'
else
    printf 'Release metadata, architecture, runner and static-binary rejection checks passed; --version was NOT exercised (set KTOP_RUNNER).\n'
fi
