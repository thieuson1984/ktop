# Package an already-built, stripped release executable without modifying it.
%global debug_package %{nil}
%global _build_id_links none
%global __os_install_post %{nil}

Name:           ktop
Version:        0.2.0
Release:        1%{?dist}
Summary:        Tiny Linux terminal system monitor
License:        MIT
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64
Packager:       ktop maintainers

%description
A lightweight Linux terminal system monitor written in Rust, reading kernel
statistics directly from procfs and sysfs without external runtime commands.
This package contains a prebuilt release executable.

%prep
%setup -q

%build
# The release executable is built with Cargo before creating the source archive.

%install
install -Dpm 0755 prebuilt/ktop %{buildroot}%{_bindir}/ktop

%files
%license LICENSE
%doc README.md
%{_bindir}/ktop

%changelog
* Thu Nov 05 2026 ktop maintainers - 0.2.0-1
- Add responsive card-based terminal dashboard and hardware inventory.
- Retain MIT licensing and the static, dependency-light release.

* Thu Nov 05 2026 ktop maintainers - 0.1.0-1
- Initial prebuilt release package.
