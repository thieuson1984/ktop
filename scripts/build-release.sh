#!/usr/bin/env bash
# Build a portable, stripped static Linux release for a supported architecture.
set -euo pipefail
export LC_ALL=C
export TZ=UTC
umask 022
root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
# shellcheck source=release-common.sh
source "$root/scripts/release-common.sh"
[[ $# -le 1 ]] || { printf 'Usage: %s [x86_64|aarch64|armv7]\n' "$0" >&2; exit 1; }
if [[ $# == 1 ]]; then
    case "$1" in
        x86_64|aarch64|armv7) arch=$1 ;;
        *) release_error 'Architecture must be x86_64, aarch64 or armv7.'; exit 1 ;;
    esac
else
    arch=$(release_arch)
fi
target=$(release_target "$arch")
version=$(release_version)
for path in Cargo.toml Cargo.lock LICENSE README.md; do
    [[ -f "$root/$path" && ! -L "$root/$path" ]] || { release_error 'Release inputs must be regular, non-symlink files.'; exit 1; }
done
for path in target target/release "target/release/$arch" dist; do
    [[ ! -L "$root/$path" ]] || { release_error 'A release output directory is a symlink.'; exit 1; }
done
linker="CARGO_TARGET_${target^^}_LINKER"
linker=${linker//-/_}
export "$linker=rust-lld"
export CARGO_TARGET_DIR="$root/target"
epoch=${SOURCE_DATE_EPOCH:-1793836800}
[[ "$epoch" =~ ^(0|[1-9][0-9]{0,9})$ ]] || { release_error 'SOURCE_DATE_EPOCH must be a nonnegative Unix timestamp (at most 10 digits).'; exit 1; }
export SOURCE_DATE_EPOCH=$epoch
tar --version | grep -F 'GNU tar' >/dev/null || { release_error 'GNU tar is required for deterministic portable archives.'; exit 1; }
# Cargo's encoded form preserves paths with spaces and cannot be bypassed by an
# inherited CARGO_ENCODED_RUSTFLAGS taking precedence over RUSTFLAGS.
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
    flags=$CARGO_ENCODED_RUSTFLAGS
else
    read -r -a words <<< "${RUSTFLAGS:-}"
    printf -v flags '%s\037' "${words[@]}"
    flags=${flags%$'\037'}
fi
for path in "${HOME:-}" "$root"; do
    [[ -n "$path" && "$path" != / ]] || continue
    destination=/build
    [[ "$path" != "$root" ]] || destination=/src/ktop
    flags+="${flags:+$'\037'}--remap-path-prefix=$path=$destination"
done
export CARGO_ENCODED_RUSTFLAGS=$flags
cargo build --locked --release --target "$target"
release_check_binary "$root/target/$target/release/ktop" "$version" static "$arch"
# Ship notices alongside the standalone binary, never inside the executable.
work=$(mktemp -d "${TMPDIR:-/tmp}/ktop-licenses.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
cargo vendor --locked --offline --respect-source-config --versioned-dirs "$work/vendor" > "$work/vendor-config"
"$root/scripts/collect-licenses.sh" "$work/vendor" "$work/licenses"
cp -- "$root/LICENSE" "$work/licenses/LICENSE-ktop"
out="$root/target/release/$arch"
mkdir -p -- "$out" "$root/dist"
cp -- "$root/target/$target/release/ktop" "$out/ktop"
rm -rf -- "$out/licenses"
mkdir -p -- "$out/licenses"
cp -R -- "$work/licenses/." "$out/licenses/"

# Stage only public distribution content; do not include local build metadata.
name="ktop-$version-linux-$arch"
mkdir -p -- "$work/archive/$name"
cp -- "$out/ktop" "$root/LICENSE" "$root/README.md" "$work/archive/$name/"
cp -R -- "$out/licenses" "$work/archive/$name/licenses"
chmod 755 "$work/archive/$name/ktop"
tar --sort=name --format=gnu --mtime="@$epoch" --owner=0 --group=0 \
    --numeric-owner --mode='u=rwX,go=rX' -C "$work/archive" -cf - "$name" \
    | gzip -n -9 > "$work/$name.tar.gz"
mv -- "$work/$name.tar.gz" "$root/dist/$name.tar.gz"
printf 'Release executable: target/release/%s/ktop; distribute with target/release/%s/licenses/\n' "$arch" "$arch"
printf 'Portable archive: dist/%s.tar.gz\n' "$name"
