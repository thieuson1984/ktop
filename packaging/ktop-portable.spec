# Binary-only packaging, deliberately separate from the offline source RPM.
%global debug_package %{nil}
%global _buildhost localhost
%global source_date_epoch_from_changelog 1
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1
%global _build_id_links none
%global __os_install_post %{nil}

Name:           ktop
Version:        0.4.0
Release:        1.portable%{?dist}
Summary:        Tiny Linux terminal system monitor (portable static build)
License:        MIT
Source0:        %{name}-%{version}-portable-%{_target_cpu}.tar.gz
ExclusiveArch:  x86_64 aarch64
Packager:       ktop maintainers
AutoReqProv:    no
BuildRequires:  coreutils

%description
A lightweight Linux terminal system monitor packaged from a prebuilt, stripped,
static musl executable. This binary-only packaging does not compile source or
run the Rust unit and real-terminal test suites. Use ktop.spec on a native host
for an offline, vendored source build with those tests.

%prep
%setup -q -n %{name}-%{version}-portable

%build
# Architecture, linkage, privacy and version are checked by build-rpm.sh before
# this archive is assembled. Preserve exactly those already verified bytes.

%install
install -Dpm 0755 ktop %{buildroot}%{_bindir}/ktop

%check
# Packaging integrity only: this is not a source-build test result.
cmp -s ktop %{buildroot}%{_bindir}/ktop

%files
%license licenses
%doc README.md
%{_bindir}/ktop

%changelog
* Fri Nov 06 2026 ktop maintainers - 0.4.0-1.portable
- Package the colorful dashboard and reversible process-table sorting release.

* Thu Nov 05 2026 ktop maintainers - 0.3.0-1.portable
- Package the explicitly verified static executable without claiming a source build.
- Keep native vendored source RPM builds and their complete tests separate.
