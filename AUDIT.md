# Project audit — 0.3.0

Baseline: release 0.2.0 (`2986fb1`). Scope: all Rust modules and tests, terminal
input/output/lifecycle, procfs/sysfs collectors, process control, dependency
metadata, release scripts, RPM/source packaging, licensing, privacy, and usage
instructions. An independent read-only reviewer examined the baseline and new
process-control boundary; runtime verification was performed by the integration
owner. This is a bounded engineering audit, not a proof that undiscovered bugs
or future dependency advisories cannot exist.

## Findings and corrections

| Severity | Area | Finding | Correction / evidence |
| --- | --- | --- | --- |
| P1 | Release source inventory | Recursive source copying could include ignored/untracked private captures and symlink text. | `scripts/build-rpm.sh` stages an approved tracked-file inventory and rejects symlinked source files/directories. No working-tree directory snapshot is published. |
| P1 | Process identity | A proc mount from another PID namespace can describe a different process than the same numeric PID passed to `pidfd_open`. | `control::Pending::open` rejects a `/proc/self` PID that differs from the caller. A rootless inherited-proc namespace smoke check exercised the denial without signalling a displayed host process. |
| P1 | Confirmation input | Dropped paste openers or embedded paste-end markers could turn pasted `y` into confirmation. | Paste cancels the immutable pending action; discarded tails preserve framing; an input-quiet barrier suppresses queued commands. Owned-child regressions cover split paste, embedded end markers, fresh `y` after cancellation, and same-burst activation. |
| P2 | Destructive UI | A tiny/resized terminal could hide the PID/action while still accepting confirmation. | Minimum-size checks at activation and confirmation; resize cancels pending actions. PTY tests verify the child remains alive after a hidden-target attempt. |
| P2 | Selection | Sorting, disappearance, and PID reuse could otherwise move an action to a different row/process. | Stable PID/start-time identity, immutable owned pidfd per pending action, invalidation on disappearance, and explicit reselection. Owned-child tests cover exit and stale identity. |
| P2 | Filter context | An active process filter persisted in overview rows without displaying its query. | Both process and overview views expose active filter context; switching views does not silently present matches as the full inventory. |
| P2 | Permission semantics | A signal-zero probe would incorrectly reject Linux's same-session SIGCONT exception. | Authorization is left to the actual confirmed pidfd signal call. No privilege escalation or numeric-PID fallback. |
| P2 | Input framing | One-byte CSI handling dispatched sequence tails as shortcuts; standalone Escape swallowed quit. | Bounded whole CSI/SS3 decoding, Escape resynchronization, timed standalone Escape, bracketed-paste isolation, and always-available raw Ctrl-C/Ctrl-D. Synthetic and PTY regressions cover the boundary. |
| P2 | Terminal lifecycle | Raw mode preceded termination handlers; suspend/resume could retain a paste state after input flushing. | Interactive handlers are installed before raw mode, snapshots retain normal signal behavior, and suspend restores/reinitializes terminal/input state. PTY journeys exercise suspend/resume and termination. |
| P2 | C ABI output buffers | On musl, `tcgetattr` can leave unused control-character slots unwritten; uninitialized storage made restoration comparisons nondeterministic. | Zero-initialized termios/statvfs output structures. The original full termios assertions were retained and pass. |
| P2 | CPU hotplug | Positional CPU baselines misidentified remaining cores after removal. | Kernel-ID keyed adjacent baselines and explicit `cpu_ids`; UI labels use the real IDs. Hotplug/reappearance/guest-accounting fixtures pass. |
| P2 | Stale samples | Failed/skipped reads retained values or divided multi-sample changes by one refresh interval. | Clear unavailable values, invalidate failed/absent baselines, require adjacent generations, and accumulate independent warnings. |
| P2 | Process parsing | Non-UTF-8 names were dropped; malformed counters could become zeros. | Bounded reusable byte reads; strict PID/state/tick/start/RSS validation; lossy name conversion only for display. Names containing spaces/parentheses and invalid bytes are covered. |
| P2 | Counter integrity | Invalid memory units, nonfinite load/uptime, and inconsistent network/disk rows looked valid. | Strict units/ranges/checked conversions; invalid rows are omitted and reported. Synthetic boundary fixtures pass. |
| P2 | I/O arithmetic | Saturating sectors-to-bytes before subtraction lost valid deltas beyond the conversion boundary. | Subtract raw sector counters first, then apply units; reject filesystem-size overflow. Reset/reorder/wrap fixtures pass. |
| P2 | ARMv7 portability | A 32-bit `statvfs` block-size field could not multiply a 64-bit block count. | Widen the block size on 32-bit targets; ARMv7 builds, Clippy, and emulated root-filesystem/process smoke checks pass. |
| P2 | Sensor validity | Faulted/disabled sensor channels were trusted; an AMD actual-clock interface was missing. | Respect optional fault/enable flags and support documented AMD hwmon frequency units alongside existing Intel/AMD paths. |
| P2 | Missing-data UI | Empty CPU/memory/device data could resemble measured zero usage. | `N/A`/unavailable states, warnings, and separate tests distinguishing missing data from genuine zeros. New-rate baselines remain explicitly documented as zero until two adjacent observations exist. |
| P2 | Source rebuild | Earlier SRPMs only carried a prebuilt executable. | Default RPM workflow builds native source frozen/offline, using vendored locked libc and a fresh Cargo home. Compiler/build prerequisites remain external toolchain requirements. |
| P2 | License completeness | The app MIT license alone did not carry bundled dependency/runtime notices. | Release artifacts include libc notices and compressed Rust library/distribution notices; application licensing remains MIT. Missing required notices fail explicitly. |
| P2 | Offline self-build / notices | Re-vendoring ignored source replacements; old Rust notice layouts do not provide the complete release inventory. | Respect Cargo source configuration during offline vendoring. Keep source compilation at Rust 1.85+, but require a current complete-notice toolchain for release assembly and explain the prerequisite instead of faking notices. |
| P2 | Release provenance | Repeated versions, unchecked architectures, and inherited flag precedence could undermine release checks/remapping. | Shared version/ELF/linkage/stripping/path checks, explicit architecture mapping, mandatory encoded path remapping, normalized metadata, and source/binary version agreement. |
| P2 | Native RPM dependencies | A minimal relocated RPM configuration could classify a dynamic executable but omit its shared-library requirements. | Native packaging now refuses output without libc ABI requirements; release builders must load complete RPM attributes/generators rather than silently publish incomplete installer metadata. |
| P3 | Rendering cost | Full styled frames were serialized every refresh. | Reuse current/previous compact cells and emit changed spans only; an incremental-readout regression verifies shorter text erases old characters. |
| P3 | Ranking/allocation cost | Every redraw allocated/sorted a fresh process-reference list. | Reuse rankings and refresh them only on samples, sort/filter changes, or actions; visible rows resolve only their selected stable identities. |
| P3 | Peak retention | Large viewports/inventories could retain oversized buffers. | Reclaim collector/ranking/cell/output allocations after shrink; a live paused-resize smoke run observed RSS return close to its initial value. |

All concrete findings above are corrected in the implementation. Unsupported
hardware counters are an explicit platform limit, not fake fallback data.

## Security model and residual limits

| Boundary / asset | Threat | Control / limit |
| --- | --- | --- |
| Kernel counters → terminal | Process/device labels inject terminal commands or disturb column geometry. | Dynamic strings are printable-ASCII sanitized; only renderer-owned single-column drawing glyphs bypass that path. |
| Terminal → mutation request | Paste, incomplete CSI, repeated keys, or reordering triggers an unintended action. | Complete bounded decoding, stable selection, immutable pending target, visible confirmation, quiet barrier, and safe cancellation. |
| Displayed PID → kernel process | Reuse/namespace mismatch signals a different task. | Namespace guard, pidfd open before start-time validation, owned descriptor through confirmation, no numeric fallback. |
| Caller → other processes | Unintended privilege escalation or hidden target. | Normal kernel permissions, protected PID/self checks, no sudo/helper/setuid, viewport guards. An operator who deliberately runs as root has root's normal authority. |
| Working tree → public artifacts | Local captures, credentials, identities, build paths, or symlinks leak. | Approved tracked source inventory, path remapping, generic packager/buildhost, normalized owners/timestamps, noreply commits, and artifact privacy inspection. |
| Registry/toolchain → release | Dependency drift, missing notices, or mutable network inputs. | Cargo.lock checksum, offline vendoring/rebuild, explicit compiler prerequisites, release preflight, and complete bundled notices. |

The quiet barrier is an accidental-input safeguard, not authentication against a
malicious same-user PTY controller. SIGTERM can be ignored by the target;
SIGKILL loses unsaved work; SIGSTOP can leave locks held. A snapshot is not atomic
across all processes/devices. SIGKILL of ktop cannot run cleanup. No persistent
action history or telemetry is added.

The locked direct dependency is `libc 0.2.191`. Inspection of the complete
[public RustSec inventory at revision
965bf9ad083b58e6340d65829d85217b18be9290](https://github.com/RustSec/advisory-db/tree/965bf9ad083b58e6340d65829d85217b18be9290/crates)
found no `crates/libc/` advisories in that snapshot. This is not a security
certification of Rust's runtime, the kernel, or future dependency versions.
Release checksums detect download corruption; unsigned packages are not a
substitute for maintainer signing/provenance. Signing-key ownership and CI trust
policy remain explicit future release work, not implicitly generated credentials.

## Exercised verification

- 44 automated Rust checks passed: 40 unit/synthetic checks and four real PTY
  journeys. Both normal process controls and negative confirmation paths use
  owned children only.
- The full 44-test suite also passed with Rust 1.85.1 for native source/MSRV
  verification. Release artifacts use current stable Rust with complete notices.
- x86-64 native plus ARM64/ARMv7 QEMU smoke checks passed for snapshots, TUI,
  exact-PID filtering, suspend/resume/kill, and terminal restoration; ARM Clippy
  checks passed. This does not assert testing on native ARM hardware.
- Disposable synthetic checkouts verified tracked-source symlink rejection and
  portable-build documentation-symlink rejection before compilation.
- Formatting and Clippy with warnings denied passed for the verified release
  candidate. Static release preflight and malformed-version/dynamic-binary
  rejection checks passed.
- Rootless inherited-proc/PID-namespace denial was exercised separately.
- PTY coverage includes suspend/resume/terminate/kill, stale/exited identity,
  queued and fragmented paste, immediate cancellation, hidden-target resize,
  quit in an activation tail, terminal restoration, and snapshot termination.
- Cross-UID SIGCONT permission combinations are enforced by the kernel but are
  not an automated local multi-UID test matrix; that expansion is in the roadmap.

Platform package/runner commands and their limitations are documented in
[README.md](README.md). A fresh source-RPM rebuild completed with an isolated
network namespace, a new Cargo home, frozen/offline Cargo, and all 44 checks.
Every extracted payload file was byte-identical to the original native RPM.
The checked RPM 6 `-ba` versus `--rebuild` containers differed only in the
source-signature reference tag and its derived header digests/size, so
whole-container byte identity is not claimed. Publication additionally requires
notice/privacy inspection and anonymous download/checksum verification.
Emulation is not evidence of testing on native ARM hardware, and no Debian-host
installer transaction or privileged system installation was performed.

## Measured runtime tradeoffs

Live-host smoke comparison, not a controlled hardware benchmark: same 112×38
PTY, 200 ms refresh, one-second warm-up, 30 consecutive 200 ms observations per
candidate. CPU used each process's scheduler-runtime delta; RSS came from its
own status; terminal bytes were counted by the harness. Other host workload and
process counts were not frozen, so CPU/RSS values are observations, not universal
speed or memory claims. No real process/device captures or host identifiers are
stored in this report.

| Observation | 0.2.0 | 0.3.0 candidate |
| --- | ---: | ---: |
| Median CPU, one-core percent | 3.497 | 2.054 |
| p95 CPU, one-core percent | 4.244 | 3.466 |
| Median terminal bytes per 200 ms | 9,174.5 | 2,640 |
| Maximum observed RSS, KiB | 652 | 724 |
| Static executable bytes | 516,504 | 543,096 |

Terminal traffic fell about 71% in this observation. The richer process/security
feature set costs about 72 KiB additional observed RSS and 26,592 executable
bytes; **overall memory/binary size did not shrink against the simpler 0.2.0
feature set**. Optimizations reduce redraw work, transient allocations, and
retained peaks rather than concealing that tradeoff. Native package size and
mandatory compressed license notices are separate from this static-binary figure.

Paused resize check, 112×38 → 512×200 → 112×38: RSS was 696 → 1,568 → 700 KiB.
This demonstrates peak display-buffer reclamation for that scenario, not a
constant-memory guarantee for arbitrarily many running processes.

## Release and future work

The application remains MIT licensed. Release history is preserved; new changes
use focused Conventional Commits and a GitHub noreply identity. Public examples
contain no captured hostname, username, full kernel release, process arguments,
or serial numbers. Future priorities and measurable exit gates are in
[ROADMAP.md](ROADMAP.md).
