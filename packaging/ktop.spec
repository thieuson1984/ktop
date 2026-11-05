# Build from complete vendored source without network access.
%global debug_package %{nil}
%global _buildhost localhost
%global source_date_epoch_from_changelog 1
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1
%global _build_id_links none
# Cargo's release profile strips the executable; preserve the verified bytes.
%global __os_install_post %{nil}
%global rust_target %{_target_cpu}-unknown-linux-gnu

Name:           ktop
Version:        0.3.0
Release:        1%{?dist}
Summary:        Tiny Linux terminal system monitor
License:        MIT
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64 aarch64
Packager:       ktop maintainers
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  glibc-devel
BuildRequires:  binutils
BuildRequires:  gzip
BuildRequires:  bash
BuildRequires:  grep
BuildRequires:  coreutils

%description
A lightweight Linux terminal system monitor written in Rust, reading kernel
statistics directly from procfs and sysfs without external runtime commands.
Built offline from vendored source for Fedora x86_64 or aarch64 and its system
glibc. Native builds run the complete Rust unit and real-terminal test suites.

%prep
%setup -q

%build
# This source contract requires native execution, including child processes in
# real-terminal tests; a Cargo runner alone cannot provide that guarantee.
test "$(uname -m)" = "%{_target_cpu}"
# SOURCE_DATE_EPOCH is normally supplied by build-rpm.sh; rebuilding the SRPM
# uses this changelog's date instead of the current wall clock.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1793836800}"
export LC_ALL=C TZ=UTC CARGO_NET_OFFLINE=true
export CARGO_HOME="$PWD/.cargo-home"
export CARGO_TARGET_DIR="$PWD/target"
# Do not inherit flags that can override path-remapping or stripping policy.
export CARGO_ENCODED_RUSTFLAGS="$(printf '%%s\037%%s' "--remap-path-prefix=${HOME:-/nonexistent}=/build" "--remap-path-prefix=$PWD=/src/ktop")"
cargo build --frozen --offline --release --target %{rust_target}
# Regenerate notices for the actual build compiler when available. Minimal
# Fedora build roots can use the archived compiler-license snapshot instead.
notices="$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html"
distribution="$(rustc --print sysroot)/share/doc/rust/COPYRIGHT.html"
if test -f "$notices" && test -f "$distribution"; then
    bash scripts/collect-licenses.sh vendor THIRD_PARTY_LICENSES
else
    printf '%%s\n' 'Using archived Rust license snapshots; review notices when changing compilers.'
fi
rustc --version > THIRD_PARTY_LICENSES/rust/BUILD-TOOLCHAIN

%check
export LC_ALL=C TZ=UTC CARGO_NET_OFFLINE=true
export CARGO_HOME="$PWD/.cargo-home" CARGO_TARGET_DIR="$PWD/target"
export CARGO_ENCODED_RUSTFLAGS="$(printf '%%s\037%%s' "--remap-path-prefix=${HOME:-/nonexistent}=/build" "--remap-path-prefix=$PWD=/src/ktop")"
# Bash is used explicitly because RPM's default build shell can be POSIX sh.
cargo test --frozen --offline --target %{rust_target}
bash -eu -o pipefail -c '
    root=$PWD
    source scripts/release-common.sh
    test "$(release_version)" = "%{version}"
    release_check_binary "$PWD/target/%{rust_target}/release/ktop" "%{version}" native "%{_target_cpu}"
'

%install
install -Dpm 0755 target/%{rust_target}/release/ktop %{buildroot}%{_bindir}/ktop

%files
%license LICENSE THIRD_PARTY_LICENSES
%doc README.md
%{_bindir}/ktop

%changelog
* Thu Nov 05 2026 ktop maintainers - 0.3.0-1
- Support native aarch64 source builds with complete unit and terminal checks.
- Build the Fedora executable offline from complete vendored source.
- Verify architecture, stripped output, versions and build-path privacy.
- Include dependency and bundled Rust standard-library license notices.
- Keep the separate portable static musl release workflow.

* Thu Nov 05 2026 ktop maintainers - 0.2.0-1
- Add responsive card-based terminal dashboard and hardware inventory.
- Retain MIT licensing and the static, dependency-light release.

* Thu Nov 05 2026 ktop maintainers - 0.1.0-1
- Initial prebuilt release package.
