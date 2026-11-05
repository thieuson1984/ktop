#!/usr/bin/env bash
# Usage: scripts/build-rpm.sh [additional rpmbuild options]
# Build target/release/ktop with Cargo --locked --release first.
set -euo pipefail
export LC_ALL=C
export TZ=UTC
umask 022

root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
version=0.2.0
name=ktop
rpmbuild=${RPMBUILD:-rpmbuild}
output=${RPM_OUTPUT_DIR:-"$root/dist"}
# Fixed release timestamp; override with the commit timestamp for another release.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1793836800}

if ! command -v "$rpmbuild" >/dev/null 2>&1; then
    printf 'rpmbuild is required; install rpm-build or set RPMBUILD.\n' >&2
    exit 1
fi
for path in Cargo.toml Cargo.lock LICENSE README.md packaging/ktop.spec target/release/ktop; do
    if [[ ! -f "$root/$path" ]]; then
        printf 'Missing release input: %s\n' "$path" >&2
        exit 1
    fi
done
if [[ ! -x "$root/target/release/ktop" ]]; then
    printf 'target/release/ktop must be an executable release binary.\n' >&2
    exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/ktop-rpm.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
stage="$work/stage/$name-$version"
mkdir -p "$stage/prebuilt" "$stage/packaging" "$stage/scripts"
# Explicit allowlist keeps local settings, build output and machine data out.
cp -- "$root/Cargo.toml" "$root/Cargo.lock" "$root/LICENSE" "$root/README.md" "$stage/"
cp -R -- "$root/src" "$stage/src"
cp -- "$root/packaging/ktop.spec" "$stage/packaging/"
cp -- "$root/scripts/build-rpm.sh" "$root/scripts/build-release.sh" "$stage/scripts/"
cp -- "$root/target/release/ktop" "$stage/prebuilt/ktop"
tar --sort=name --format=gnu --mtime="@$SOURCE_DATE_EPOCH" \
    --owner=0 --group=0 --numeric-owner --mode='u+rwX,go+rX,go-w' \
    -C "$work/stage" -cf - "$name-$version" |
    gzip -n -9 > "$work/SOURCES/$name-$version.tar.gz"
cp -- "$root/packaging/ktop.spec" "$work/SPECS/"

"$rpmbuild" "$@" -ba "$work/SPECS/ktop.spec" \
    --target x86_64 \
    --define "_topdir $work" \
    --define 'dist .fc45' \
    --define '_buildhost localhost' \
    --define 'packager ktop maintainers' \
    --define 'use_source_date_epoch_as_buildtime 1' \
    --define 'clamp_mtime_to_source_date_epoch 1'
mkdir -p "$output"
cp -- "$work"/RPMS/x86_64/*.rpm "$work"/SRPMS/*.rpm \
    "$work/SOURCES/$name-$version.tar.gz" "$output/"
printf 'RPMs and source archive written to %s\n' "$output"
