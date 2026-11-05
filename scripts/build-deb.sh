#!/usr/bin/env bash
# Package an existing canonical static release; never builds or installs ktop.
# Usage: scripts/build-deb.sh [x86_64|aarch64|armv7]
set -euo pipefail
export LC_ALL=C
export TZ=UTC
umask 022

root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# All inputs must stay below the physical repository root without symlink hops.
deb_check_input() {
    local relative=$1 parent=$1
    [[ -f "$root/$relative" && ! -L "$root/$relative" ]] || {
        printf 'Debian packaging error: missing, nonregular or symlinked input: %s\n' "$relative" >&2
        return 1
    }
    while [[ "$parent" == */* ]]; do
        parent=${parent%/*}
        [[ ! -L "$root/$parent" ]] || {
            printf 'Debian packaging error: a release input directory is a symlink.\n' >&2
            return 1
        }
    done
}

deb_check_input scripts/release-common.sh
deb_check_input Cargo.toml
# shellcheck source=release-common.sh
source "$root/scripts/release-common.sh"
[[ $# -le 1 ]] || { release_error 'Usage: scripts/build-deb.sh [x86_64|aarch64|armv7]'; exit 1; }
arch=$(release_arch "${1:-$(uname -m)}")
case "$arch" in
    x86_64) deb_arch=amd64 ;;
    aarch64) deb_arch=arm64 ;;
    armv7) deb_arch=armhf ;;
esac
version=$(release_version)
dpkg_deb=${DPKG_DEB:-dpkg-deb}
# Match the release scripts' reproducible baseline, not the current wall clock.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH-1793836800}
[[ "$SOURCE_DATE_EPOCH" =~ ^(0|[1-9][0-9]{0,9})$ ]] || { release_error 'SOURCE_DATE_EPOCH must be a nonnegative Unix timestamp (at most 10 digits).'; exit 1; }
for tool in "$dpkg_deb" readelf strings grep install mktemp mkdir gzip touch find sort xargs md5sum du mv rm; do
    command -v "$tool" >/dev/null 2>&1 || { release_error "$tool is required for Debian packaging."; exit 1; }
done

artifact="target/release/$arch"
binary="$root/$artifact/ktop"
deb_check_input "$artifact/ktop"
deb_check_input packaging/debian/control.in
deb_check_input packaging/debian/copyright
# Deliberately allowlist every notice: never sweep a working tree or a directory
# that might also contain captures, temporary files or other private content.
notices=(
    LICENSE-ktop
    libc/LICENSE-MIT
    libc/LICENSE-APACHE
    rust/COPYRIGHT-library.html.gz
    rust/COPYRIGHT-distribution.html.gz
    rust/TOOLCHAIN
)
for notice in "${notices[@]}"; do
    deb_check_input "$artifact/licenses/$notice"
done
if [[ -e "$root/README.md" || -L "$root/README.md" ]]; then
    deb_check_input README.md
fi
[[ ! -L "$root/dist" && ( ! -e "$root/dist" || -d "$root/dist" ) ]] || { release_error 'dist must be a real directory, not a symlink.'; exit 1; }
release_check_binary "$binary" "$version" static "$arch"

work=$(mktemp -d "${TMPDIR:-/tmp}/ktop-deb.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
stage="$work/package"
doc="$stage/usr/share/doc/ktop"
mkdir -p "$stage/DEBIAN" "$doc/licenses"
install -D -m 0755 -- "$binary" "$stage/usr/bin/ktop"
install -m 0644 -- "$root/packaging/debian/copyright" "$doc/copyright"
for notice in "${notices[@]}"; do
    install -D -m 0644 -- "$root/$artifact/licenses/$notice" "$doc/licenses/$notice"
done
if [[ -f "$root/README.md" ]]; then
    gzip -n -9 -c -- "$root/README.md" > "$doc/README.md.gz"
fi
read -r installed_size _ < <(du -sk -- "$stage/usr")
while IFS= read -r line || [[ -n "$line" ]]; do
    line=${line//@VERSION@/$version}
    line=${line//@ARCH@/$deb_arch}
    line=${line//@INSTALLED_SIZE@/$installed_size}
    printf '%s\n' "$line"
done < "$root/packaging/debian/control.in" > "$stage/DEBIAN/control"
(cd "$stage" && find usr -type f -print0 | sort -z | xargs -0 md5sum) > "$stage/DEBIAN/md5sums"
# dpkg-deb clamps archive timestamps; normalize all staged entries too, including
# directories and generated control/checksum files, for deterministic inputs.
find "$stage" -exec touch -h -d "@$SOURCE_DATE_EPOCH" -- {} +
filename="ktop_${version}_${deb_arch}.deb"
# gzip works on older dpkg versions too and has no host-dependent thread count.
# Numeric root ownership is written into the archive without fakeroot or sudo.
"$dpkg_deb" --build --root-owner-group -Zgzip -z9 "$stage" "$work/$filename"
mkdir -p "$root/dist"
mv -- "$work/$filename" "$root/dist/$filename"
printf 'Debian binary package: dist/%s\n' "$filename"
