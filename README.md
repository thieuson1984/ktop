# ktop

A small, read-only Linux system monitor with a terminal interface. Written in
Rust; reads kernel counters directly, without a service, sensor daemon, vendor
SDK, or external runtime commands. MIT licensed.

## Status displayed

- Total and per-core CPU usage, CPU0 frequency, load averages, and uptime.
- RAM used/available, swap usage, and root filesystem usage.
- Per-process CPU, resident memory, state, and thread count. Sort by CPU, memory,
  or PID; scroll through the process list. CPU usage is **100% per busy core**,
  so multithreaded processes can exceed 100%. Initial/new processes need two
  observations to establish a CPU rate. Process names, not command arguments,
  are shown.
- Network receive/transmit and whole-device disk read/write rates.
- Temperatures, voltages, fan speeds, and power from Linux hwmon.
- GPU utilization, VRAM, clocks, temperature, and power when exported through
  DRM/sysfs. AMD counters and Intel i915/Xe clock interfaces are supported;
  NVIDIA support is limited to what its driver exports in sysfs. Missing GPU
  counters display `unavailable`; there is no estimated or fabricated fallback.

Hardware availability depends on the kernel, driver, hardware, and permissions.
No root privileges are required. Some sensors may remain inaccessible; ktop
does not load kernel modules or change device permissions. Hardware refreshes
at most once every two seconds; other counters use the chosen refresh period.
I/O rates omit loopback, loop/RAM disks, device-mapper devices, and partitions;
shared process pages can appear in more than one process's RSS. CPU samples
exclude guest-time double counting and treat I/O wait as idle.

## Run

Use a Linux terminal supporting ANSI cursor positioning and the alternate screen.
The minimum terminal size is 42 columns by 12 rows; wider terminals show more
columns. Long rows are clipped. The hardware page allows scrolling through all
readable sensors.

```sh
ktop
ktop --interval 2
ktop --help
ktop --once
```

`--interval` accepts 0.2–60 seconds. `--once` waits one interval, prints a local
snapshot, and exits; it does not create a file. Interactive mode restores the
terminal on exit, Ctrl-C, SIGINT, SIGTERM, and SIGHUP. SIGKILL cannot be handled;
use `reset` if an uncatchable termination leaves a terminal in raw mode.

| Key | Action |
| --- | --- |
| `1` / `2` / `3` | Overview / hardware / processes |
| `c` / `m` / `p` | Sort by CPU / resident memory / PID |
| `j` / `k`, Down / Up | Scroll |
| `g` | Scroll to top |
| Space | Pause/resume collection |
| `+` / `-` | Faster/slower refresh |
| `q`, Ctrl-C, Ctrl-D | Quit |

## Build

Rust 1.85 or newer is required. The only direct runtime crate is `libc`.
Release settings prioritize size: `opt-level = "z"`, LTO, one codegen unit,
stripped symbols, and abort-on-panic. Memory is proportional to the number of
processes, not uptime: ktop keeps current counters only, without history files.

For the portable x86_64 static release, install Rust with rustup and a host C
linker/toolchain (Fedora: `gcc` and `glibc-devel`), then:

```sh
rustup target add x86_64-unknown-linux-musl
scripts/build-release.sh
```

The script uses Rust's bundled LLD for the target, remaps home/project build
paths, and places the stripped executable at `target/release/ktop`. The resulting
binary does not need musl installed on the destination. Cargo still needs a
host linker for dependency build scripts. First-time Cargo builds download
locked dependencies; the running application never connects to the network.

For a native dynamically linked development build:

```sh
cargo build
cargo test
```

## Fedora 45 RPM

Install `rpm-build` and `redhat-rpm-config` for packaging. Build the release above,
then:

```sh
scripts/build-rpm.sh
```

Outputs in `dist/`:

- `ktop-0.1.0-1.fc45.x86_64.rpm` — installable unsigned binary RPM.
- `ktop-0.1.0-1.fc45.src.rpm` — packaging/source RPM, including the prebuilt binary.
- `ktop-0.1.0.tar.gz` — normalized source archive with the prebuilt binary.

This is **prebuilt-binary packaging**, not a Fedora distribution submission or a
source-compiling Fedora SRPM. The spec installs the binary, README, and license
only: no daemon, scripts run at installation, or configuration files.
Packaging requires no root. Installation requires administrator privileges:

```sh
sudo dnf install ./dist/ktop-0.1.0-1.fc45.x86_64.rpm
```

`RPMBUILD` selects a different rpmbuild executable; `RPM_OUTPUT_DIR` selects the
artifact directory; `SOURCE_DATE_EPOCH` overrides the release timestamp. Generated
RPM metadata uses a generic build host and packager. Reproducibility requires the
same locked dependencies, Rust toolchain, and RPM/compression tools.

## Privacy

Metrics stay in the local terminal. ktop has no telemetry, upload, persistent
logs, or configuration storage. It does not read hostnames, user names, process
command arguments/environments, IP addresses, or hardware serial numbers.
Process names and device/interface names are still local system information:
do not publish real screenshots or `--once` output without reviewing them.
Control characters in device/process names are sanitized before display.

Only source, packaging scripts, and generic documentation belong in Git.
Build output and RPMs are ignored. Release archives use an explicit input list,
normalized ownership, and build-path remapping. Commit identities should use a
GitHub noreply address, not a workstation's local Git identity.

## Release 0.1.0

Initial Linux TUI with procfs CPU/memory/process/I/O statistics, hwmon sensors,
DRM GPU counters, bounded refresh controls, and Fedora 45 x86_64 RPM packaging.
