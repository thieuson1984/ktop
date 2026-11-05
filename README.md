# ktop

A compact Linux system monitor and process manager with a card-based terminal
interface. Written in Rust with one direct dependency, `libc`. Reads procfs and
sysfs directly: no daemon, sensor service, vendor SDK, network requests, task
database, or persistent logs. [MIT licensed](LICENSE).

## Monitor

- Total/per-core CPU usage with stable kernel CPU IDs, CPU0 frequency, load, and
  uptime. Guest CPU time is not double-counted; I/O wait counts as idle.
- RAM used/available, swap, and root filesystem usage.
- Per-process CPU, resident memory, state, and thread count. **100% CPU means one
  busy core**; multithreaded processes can exceed 100%. New processes/counters
  need two adjacent observations to establish a rate; the baseline is zero.
- Aggregate network RX/TX and whole-device disk read/write rates. Loopback,
  loop/RAM disks, device-mapper devices, and partitions are omitted. Shared pages
  can appear in more than one process's RSS.
- hwmon temperatures, raw voltages, fan speeds, and power, respecting optional
  sensor enable/fault flags. Voltage is the kernel's raw channel value: ktop does
  not apply board-specific libsensors resistor scaling.
- GPU utilization, VRAM, clocks, temperature, and power when exported through
  DRM/sysfs. AMD and Intel i915/Xe interfaces are supported; NVIDIA is limited to
  what its driver exports. Unsupported counters say `unavailable`, not estimates.

Sampling is best-effort, not an atomic machine-wide snapshot. Invalid/missing
samples clear stale data, invalidate interrupted rate baselines, and show a
warning. Missing CPU/memory/device readouts display `N/A` rather than claiming
zero usage. No root privileges are needed; restricted kernel counters remain
unavailable. Hardware refreshes at most once every two seconds.

## Interface

Use a UTF-8 Linux terminal with ANSI cursor positioning, alternate-screen support,
and single-column box-drawing characters. Minimum size: **42 columns × 12 rows**.
Cards adapt to the viewport; wide layouts add per-core and hardware/traffic
panels. Overflowing labels use ellipses, while numeric columns retain their own
space. The hardware inventory exposes every collected sensor/GPU field and CPU
core. Compact `K/M/G/T` labels mean KiB/MiB/GiB/TiB.

Colors use the terminal's palette and default background. `NO_COLOR=1` disables
styling; tabs, status labels, borders, selection marker, and confirmation text
remain meaningful without color. Only changed terminal spans are transmitted.

```sh
ktop
ktop --interval 2
ktop --help
ktop --once
```

`--interval` accepts 0.2–60 seconds. `--once` waits one interval and prints a
read-only, ANSI-free UTF-8 snapshot; it never signals processes or creates a file.

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Overview / hardware / processes |
| `c` / `m` / `p` | CPU / resident-memory / PID sort |
| `j` / `k`, Down / Up | Scroll; select a process in view 3 |
| Page Up / Page Down | Move by a page in view 3 |
| `g` / `G`, Home / End | First / last item |
| `/` | Edit process filter: case-insensitive name or exact numeric PID |
| Enter / Esc | Apply / cancel filter edit |
| Ctrl-U / Backspace | Clear / edit filter text; apply an empty filter to reset |
| `t` / `K` / `s` / `r` | Request terminate / force-kill / suspend / resume in view 3 |
| `y` / Esc / `n` | Confirm / cancel a pending process action |
| Space | Pause/resume collection |
| `+` / `-` | Faster/slower refresh |
| `q`, Ctrl-C, Ctrl-D | Quit (`q` is text while editing a filter) |
| Ctrl-Z | Restore the terminal and suspend ktop; shell `fg` resumes it |

An applied process filter persists in the process and overview lists; both show
the active query and match count. Apply an empty filter to return to all tasks.
Filters accept up to 64 printable ASCII characters; use an exact PID for names
that cannot be entered directly.

## Process-management safety

1. Open view 3 and select a process, or filter to its exact PID.
2. Request an action. The dialog freezes the selected PID, start-time identity,
   name, action, and warning; sorting/refresh cannot retarget it.
3. Review the visible target, then press `y` to send its signal. Esc or `n`
   cancels. A successful send does not guarantee that a process obeys SIGTERM.

Actions use `pidfd_open` and `pidfd_send_signal`; there is **no `kill(pid)`
fallback**. A start-time check rejects stale selection, and `/proc/self` must
match the caller's PID namespace. Descriptor ownership prevents retargeting
while a confirmation is pending. PID 0, PID 1, and ktop itself are protected.
The kernel enforces normal permissions on the actual confirmed signal, including
SIGCONT's same-session rules. ktop never requests sudo or installs a privileged
helper. Run it unprivileged unless elevated process access is truly intended.

Control requires a kernel with pidfd support (Linux 5.3+); unsupported kernels,
permission failures, exits, and namespace mismatches fail closed. Shrinking or
resizing the terminal cancels a pending action, and actions cannot be confirmed
below the minimum viewport. Bracketed paste is ignored and cancels any pending
action; complete escape sequences cannot become shortcuts. Queued activation
and confirmation bytes are discarded while retaining protocol framing. A short
post-render quiet barrier
protects against accidental queued confirmation; **it is not authentication
against a malicious controller of the same terminal**.

Suspend can leave application locks held. SIGKILL cannot save unsaved work.
These are operator decisions, not automatic recovery actions. Action feedback
is temporary and no action history is silently retained.

Interactive exit, Ctrl-C, SIGINT, SIGTERM, SIGHUP, SIGQUIT, and ordinary
suspend/resume restore terminal state. SIGKILL cannot be handled; use `reset` if
an uncatchable termination leaves a terminal in raw mode.

## Supported packages and architectures

[Release downloads](https://github.com/thieuson1984/ktop/releases) include checksums,
complete license notices, and the package forms verified for that release.

| CPU | Build argument | Debian-family package | Fedora 45 RPM | Portable Linux archive |
| --- | --- | --- | --- | --- |
| x86-64 | `x86_64` | `amd64` | Native source build | Yes |
| ARM64 / ARMv8 | `aarch64` | `arm64` | Native source build or explicit portable-binary RPM | Yes |
| ARMv7 hard-float | `armv7` | `armhf` | Not a Fedora 45 target | Yes |

The static `.deb` packages suit Debian, Ubuntu, and compatible Debian-based
systems without a glibc-version dependency. Portable archives cover other Linux
distributions on these CPUs, including systems without RPM/dpkg. This is not a
claim of compatibility with every ARM generation, Android, macOS, or Windows.
ARM release smoke checks may use QEMU: emulation is not native hardware testing.
Installer transactions on Debian hosts are distinct from package-content checks.

## Self-build: common prerequisites

The application supports Rust 1.85+ with Cargo. For the release/package scripts,
use **current Rust stable** (verified with 1.99.1), whose distribution provides
complete `COPYRIGHT-library.html` and `COPYRIGHT.html` inventories. Older Rust
can compile/test the app but its legacy notice format is rejected for release
assembly rather than silently publishing incomplete bundled notices.

A host C compiler/linker, Git, Bash, GNU tar/coreutils/findutils, binutils, grep,
and gzip are required. Keep matching Rust documentation installed:

```sh
rustup toolchain install stable
rustup component add rust-docs --toolchain stable
export RUSTUP_TOOLCHAIN=stable
```

This selects the release toolchain for the current shell without changing the
global rustup default. First builds can download locked dependencies;
the running application never uses networking.

Typical native prerequisites, before installing Rust with rustup:

```sh
# Fedora:
sudo dnf install gcc glibc-devel binutils git tar gzip
# Debian/Ubuntu:
sudo apt install build-essential binutils git tar gzip
```

From a fresh checkout:

```sh
git clone https://github.com/thieuson1984/ktop.git
cd ktop
cargo build --locked
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests include synthetic collector/parser boundaries and real PTY journeys that
only signal owned children. Release settings prioritize size: optimization,
LTO, one codegen unit, stripping, and abort-on-panic. Memory grows with current
process/device counts and bounded viewport size, not elapsed uptime; rankings,
read buffers, and compact terminal cells are reused and peak allocations reclaimed.

### Native portable build

Choose the row matching the machine that will run ktop. The architecture
argument is optional when building for the current host.

```sh
# x86-64:
rustup target add x86_64-unknown-linux-musl
scripts/build-release.sh x86_64
scripts/test-release.sh target/release/x86_64/ktop x86_64
./target/release/x86_64/ktop

# On an ARM64 host:
rustup target add aarch64-unknown-linux-musl
scripts/build-release.sh aarch64

# On an ARMv7 hard-float host:
rustup target add armv7-unknown-linux-musleabihf
scripts/build-release.sh armv7
```

Outputs: `target/release/ARCH/ktop`, its sibling `licenses/`, and
`dist/ktop-0.3.0-linux-ARCH.tar.gz`. Distribute the entire portable archive, or
both the executable and its licenses directory. No musl installation is needed
on the destination. The archive contains only the executable, README, and notices.

### Cross-build ARM on x86-64

Rust's bundled LLD links the pure-Rust musl targets; no target C compiler/sysroot
is required. A host C linker is still needed for dependency build scripts.
Install a QEMU user-mode runner using the host distribution's `qemu-user` or
`qemu-user-static` packages if runtime checks are desired.

```sh
rustup target add aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf
KTOP_RUNNER=qemu-aarch64-static scripts/build-release.sh aarch64
KTOP_RUNNER=qemu-arm-static scripts/build-release.sh armv7
KTOP_RUNNER=qemu-aarch64-static scripts/test-release.sh target/release/aarch64/ktop aarch64
KTOP_RUNNER=qemu-arm-static scripts/test-release.sh target/release/armv7/ktop armv7
```

`KTOP_RUNNER` is one executable name/path, not a shell command. If the host only
has non-static `qemu-aarch64`/`qemu-arm` executables, use those names instead.
Without a runner, cross-builds perform **structural-only** ELF checks and warn
that `--version` was not executed. Published cross artifacts require actual
runner smoke evidence, not that structural warning being mistaken for a pass.

## Debian/Ubuntu packages

Install `dpkg-deb` (normally provided by dpkg; `dpkg-dev` supplies development
tools). Package a canonical static build without root:

```sh
scripts/build-deb.sh x86_64
sudo apt install ./dist/ktop_0.3.0_amd64.deb

# On an ARM host, use its architecture; for cross packaging retain the runner:
KTOP_RUNNER=qemu-aarch64-static scripts/build-deb.sh aarch64
KTOP_RUNNER=qemu-arm-static scripts/build-deb.sh armv7
```

Outputs: `ktop_0.3.0_amd64.deb`, `ktop_0.3.0_arm64.deb`, or
`ktop_0.3.0_armhf.deb` in `dist/`. Packages preserve the static executable,
install mode 0755 with root ownership, include application/dependency/runtime
notices under `/usr/share/doc/ktop/`, and have no shared-library dependency,
maintainer scripts, daemon, privileged helper, or setuid permission.

## Fedora 45 RPM and offline source RPM

For packaging, additionally install `rpm-build`, `redhat-rpm-config`, and the
declared Rust/Cargo/native compiler build prerequisites. Initial source
packaging needs locked libc cached (`cargo fetch --locked`) and matching Rust
copyright notices. Missing prerequisites fail explicitly; scripts never install
packages or silently fetch documentation.

Stage intended sources/docs/scripts with `git add` first. The default wrapper
copies only approved **tracked** inputs, rejects symlinks, derives/checks the
version, vendors the locked dependency offline, and builds frozen native source:

```sh
# Native x86-64 Fedora:
scripts/build-rpm.sh x86_64
sudo dnf install ./dist/ktop-0.3.0-1.fc45.x86_64.rpm

# Native ARM64 Fedora:
scripts/build-rpm.sh aarch64

# Rebuild complete source on a matching native Fedora host:
rpmbuild --rebuild --target x86_64 ./dist/ktop-0.3.0-1.fc45.src.rpm
```

Native outputs are `ktop-0.3.0-1.fc45.ARCH.rpm`,
`ktop-0.3.0-1.fc45.src.rpm`, and `ktop-0.3.0.tar.gz`. The source archive contains
vendored libc, Cargo configuration, tests, build scripts, and notices, **not a
prebuilt executable**. Native RPMs link to Fedora's glibc/libgcc; complete
unit/PTY suites and binary preflight run inside the source build. A fresh Cargo
home and frozen/offline mode prevent registry access. Source mode requires a
matching native host and refuses `--nocheck`; a Cargo runner alone cannot execute
all nested PTY child processes as a native test environment.

For cross-packaged static ARM64, use the separate explicit binary-only mode:

```sh
KTOP_RUNNER=qemu-aarch64-static scripts/build-rpm.sh --portable-binary aarch64
sudo dnf install ./dist/ktop-0.3.0-1.portable.fc45.aarch64.rpm
```

That mode creates **only** a `1.portable.fc45` binary RPM from the verified
canonical static build. It checks packaging integrity but does not claim source
compilation, native ARM tests, or an ARM-specific SRPM. The complete default SRPM
can be rebuilt on native ARM64 with `--target aarch64`. Neither RPM mode installs
a daemon, configuration, setuid binary, or installation scriptlets.

`RPMBUILD`/`DPKG_DEB` select tools, `RPM_OUTPUT_DIR` selects RPM output, and
`SOURCE_DATE_EPOCH` sets the normalized release timestamp. Metadata uses generic
maintainer/buildhost fields. Matching toolchains, source, compression tools,
and settings are required for byte reproducibility. Source-RPM rebuilds use
archived notices if the compiler lacks them, record the build compiler, and
require license review when distributions change. In the checked RPM 6 build
modes, rebuilt payloads were byte-identical but the container differed by its
source-signature reference and derived header digests; whole-RPM byte identity
is not claimed. See [AUDIT.md](AUDIT.md).


## Privacy and licensing

Metrics stay in the local terminal. No hostnames, usernames, process command
arguments/environments, IP addresses, or hardware serial numbers are collected.
Process/device/interface names and PIDs are still local system information:
review screenshots and snapshots before publishing them. Displayed names are
sanitized against terminal escape/control injection. No real captures belong
in commits, README examples, or source archives.

The application is [MIT licensed](LICENSE). Bundled dependency/runtime notices
are included with release artifacts; their own licenses remain applicable.
Commits use a GitHub noreply identity. Build output/RPMs are Git-ignored, and
release artifacts are checked for private paths and identifiers before publication.

## Audit, roadmap, and releases

- [Audit findings, verification, security model, and measured tradeoffs](AUDIT.md)
- [Prioritized roadmap and future release gates](ROADMAP.md)

### 0.3.0

Confirmed PID-safe process management; bounded keyboard/paste decoding; terminal
lifecycle fixes; strict collector/hotplug/rate handling; unavailable-data states;
partial-frame rendering and reusable rankings; offline source-built Fedora
RPM/SRPM; Debian packages and portable x86-64/ARM64/ARMv7 builds; complete license
notices and hardened release input checks.

### Earlier releases

0.2.0 introduced the responsive card dashboard. 0.1.0 introduced procfs/sysfs
monitoring and prebuilt Fedora RPM packaging; those prebuilt-source packages are
superseded by the source-building workflow above.
