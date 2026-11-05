#!/usr/bin/env bash
# Usage: scripts/build-rpm.sh [x86_64|aarch64] [additional rpmbuild options]
#        scripts/build-rpm.sh --portable-binary [x86_64|aarch64] [rpmbuild options]
# Default: native Fedora RPM + SRPM from complete, offline-vendored source.
# Explicit portable mode: binary RPM only from a verified canonical static build.
set -euo pipefail
export LC_ALL=C
export TZ=UTC
umask 022

root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
rpm_check_input() {
    local path=$1 parent=$1
    [[ -f "$root/$path" && ! -L "$root/$path" ]] || { printf 'RPM packaging error: missing, nonregular or symlinked input: %s\n' "$path" >&2; return 1; }
    while [[ "$parent" == */* ]]; do
        parent=${parent%/*}
        [[ ! -L "$root/$parent" ]] || { printf 'RPM packaging error: an input directory is a symlink.\n' >&2; return 1; }
    done
}
rpm_check_input scripts/release-common.sh
rpm_check_input Cargo.toml
# shellcheck source=release-common.sh
source "$root/scripts/release-common.sh"
mode=source
if [[ "${1:-}" == --portable-binary ]]; then
    mode=portable
    shift
fi
host=$(release_arch)
arch=$host
if [[ $# -gt 0 && "$1" != -* ]]; then
    arch=$(release_arch "$1")
    shift
fi
case "$arch" in
    x86_64|aarch64) ;;
    *) release_error 'Fedora RPMs support x86_64 and aarch64; use a portable tarball or Debian package for armv7.'; exit 1 ;;
esac
if [[ "$mode" == source && "$arch" != "$host" ]]; then
    release_error 'Source RPM builds must run on the target architecture so the complete unit and PTY tests execute natively. Use --portable-binary for an explicitly prebuilt cross package.'
    exit 1
fi
version=$(release_version)
if [[ "$mode" == source ]]; then
    for argument in "$@"; do
        [[ "$argument" != --nocheck ]] || { release_error 'Native source RPM packaging requires the complete unit and PTY tests; --nocheck is not supported.'; exit 1; }
    done
fi
name=ktop
rpmbuild=${RPMBUILD:-rpmbuild}
output=${RPM_OUTPUT_DIR:-"$root/dist"}
spec=packaging/ktop.spec
[[ "$mode" == source ]] || spec=packaging/ktop-portable.spec
# 2026-11-05 00:00:00 UTC: stable release baseline, never the wall clock.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1793836800}
[[ "$SOURCE_DATE_EPOCH" =~ ^(0|[1-9][0-9]{0,9})$ ]] || { release_error 'SOURCE_DATE_EPOCH must be a nonnegative Unix timestamp (at most 10 digits).'; exit 1; }
for tool in "$rpmbuild" rpm tar gzip git; do
    command -v "$tool" >/dev/null 2>&1 || { release_error "$tool is required for RPM packaging."; exit 1; }
done
for path in Cargo.lock LICENSE README.md "$spec"; do
    rpm_check_input "$path"
done
spec_version=
while IFS= read -r line; do
    if [[ "$line" =~ ^Version:[[:space:]]+([^[:space:]]+)[[:space:]]*$ ]]; then
        [[ -z "$spec_version" ]] || { release_error 'Duplicate RPM spec version.'; exit 1; }
        spec_version=${BASH_REMATCH[1]}
    fi
done < "$root/$spec"
[[ "$version" == "$spec_version" ]] || { release_error 'Cargo and RPM spec versions do not match.'; exit 1; }
parent=$output
while [[ -n "$parent" && "$parent" != / ]]; do
    [[ ! -L "$parent" ]] || { release_error 'An RPM output directory is a symlink.'; exit 1; }
    [[ "$parent" == */* ]] || break
    parent=${parent%/*}
done

work=$(mktemp -d "${TMPDIR:-/tmp}/ktop-rpm.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
if [[ "$mode" == source ]]; then
    for tool in cargo rustc; do
        command -v "$tool" >/dev/null 2>&1 || { release_error "$tool is required for source packaging."; exit 1; }
    done
    stage="$work/stage/$name-$version"
    mkdir -p "$stage/packaging" "$stage/scripts" "$stage/.cargo"
    # An approved tracked inventory only: never sweep captures or private data.
    # Root Rust sources are permitted only when tracked, like build.rs.
    git -C "$root" ls-files -z -- \
        Cargo.toml Cargo.lock LICENSE README.md AUDIT.md ROADMAP.md \
        packaging/ktop.spec packaging/ktop-portable.spec \
        packaging/debian/control.in packaging/debian/copyright \
        scripts/build-rpm.sh scripts/build-release.sh scripts/build-deb.sh \
        scripts/release-common.sh scripts/collect-licenses.sh scripts/test-release.sh \
        ':(glob)*.rs' ':(glob)src/**/*.rs' ':(glob)tests/**/*.rs' \
        ':(glob)benches/**/*.rs' ':(glob)examples/**/*.rs' > "$work/source-files"
    while IFS= read -r -d '' path; do
        rpm_check_input "$path"
        mkdir -p "$stage/$(dirname -- "$path")"
        cp -- "$root/$path" "$stage/$path"
    done < "$work/source-files"
    for path in Cargo.toml Cargo.lock LICENSE README.md src/main.rs packaging/ktop.spec \
        packaging/ktop-portable.spec packaging/debian/control.in packaging/debian/copyright \
        scripts/build-rpm.sh scripts/build-release.sh scripts/build-deb.sh \
        scripts/release-common.sh scripts/collect-licenses.sh scripts/test-release.sh; do
        [[ -f "$stage/$path" ]] || { release_error "Release input must be tracked with git add: $path"; exit 1; }
    done
    for path in AUDIT.md ROADMAP.md; do
        [[ ! -e "$root/$path" && ! -L "$root/$path" || -f "$stage/$path" ]] || { release_error "Release document must be tracked with git add: $path"; exit 1; }
    done
    # Fetch locked dependencies beforehand; packaging itself never uses network.
    (cd "$root" && cargo vendor --locked --offline --respect-source-config --versioned-dirs "$stage/vendor") > "$work/vendor-config"
    cat > "$stage/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"
[source.vendored-sources]
directory = "vendor"
[net]
offline = true
EOF
    "$root/scripts/collect-licenses.sh" "$stage/vendor" "$stage/THIRD_PARTY_LICENSES"
    archive="$name-$version.tar.gz"
    build_mode=-ba
else
    artifact="target/release/$arch"
    rpm_check_input "$artifact/ktop"
    notices=(LICENSE-ktop libc/LICENSE-MIT libc/LICENSE-APACHE \
        rust/COPYRIGHT-library.html.gz rust/COPYRIGHT-distribution.html.gz rust/TOOLCHAIN)
    for notice in "${notices[@]}"; do
        rpm_check_input "$artifact/licenses/$notice"
    done
    release_check_binary "$root/$artifact/ktop" "$version" static "$arch"
    stage="$work/stage/$name-$version-portable"
    mkdir -p "$stage/licenses"
    cp -- "$root/$artifact/ktop" "$root/README.md" "$stage/"
    for notice in "${notices[@]}"; do
        mkdir -p "$stage/licenses/$(dirname -- "$notice")"
        cp -- "$root/$artifact/licenses/$notice" "$stage/licenses/$notice"
    done
    archive="$name-$version-portable-$arch.tar.gz"
    build_mode=-bb
    printf '%s\n' 'Portable binary RPM only: no source RPM, source compilation or unit/PTY test execution is claimed.'
fi
# Normalize ownership, mode and timestamps without recording local metadata.
tar --sort=name --format=gnu --mtime="@$SOURCE_DATE_EPOCH" \
    --owner=0 --group=0 --numeric-owner --mode='u+rwX,go+rX,go-w' \
    -C "$work/stage" -cf - "${stage##*/}" |
    gzip -n -9 > "$work/SOURCES/$archive"
cp -- "$root/$spec" "$work/SPECS/"
# Literal percent signs in temporary paths must not become RPM macros.
rpm_work=${work//%/%%}
"$rpmbuild" "$@" "$build_mode" "$work/SPECS/${spec##*/}" \
    --target "$arch" \
    --define "_topdir $rpm_work" \
    --define 'dist .fc45' \
    --define '_buildhost localhost' \
    --define 'packager ktop maintainers' \
    --define 'use_source_date_epoch_as_buildtime 1' \
    --define 'clamp_mtime_to_source_date_epoch 1'
if [[ "$mode" == source ]]; then
    for package in "$work"/RPMS/"$arch"/*.rpm; do
        requirements=$(rpm -qp --queryformat '[%{REQUIRENAME}\n]' "$package")
        if ! grep -E '^libc\.so\.6' <<< "$requirements" >/dev/null; then
            release_error 'Native RPM lacks ELF runtime requirements; repair RPM file attributes/dependency generators before releasing.'
            exit 1
        fi
    done
fi
mkdir -p "$output"
cp -- "$work"/RPMS/"$arch"/*.rpm "$output/"
if [[ "$mode" == source ]]; then
    cp -- "$work"/SRPMS/*.rpm "$work/SOURCES/$archive" "$output/"
    printf 'Native %s RPM, SRPM and offline source archive written to %s\n' "$arch" "$output"
else
    printf 'Portable %s binary RPM written to %s (no SRPM)\n' "$arch" "$output"
fi
