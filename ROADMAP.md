# Roadmap

Scope: a small Linux terminal monitor and process manager. MIT licensed. Keep
kernel interfaces, unprivileged operation, local-only metrics, and a small
runtime dependency set as the defaults. Personal to-do lists, cloud accounts,
remote control, and a telemetry service are not part of this product direction.

## 0.3 delivery baseline

- Stable PID/start-time selection, exact-PID or case-insensitive name filtering,
  and confirmed terminate, force-kill, suspend, and resume actions.
- Descriptor-only signalling with namespace checks and kernel permission
  enforcement; no numeric-PID fallback, implicit sudo, or background agent.
- Whole-sequence input decoding, bracketed-paste isolation, visible confirmations,
  safe cancellation, terminal suspension/restoration, and partial-frame updates.
- Strict counter parsing, hotplug-aware CPU IDs, interrupted-rate invalidation,
  hwmon validity flags, and retained-buffer reclamation.
- Offline source-built Fedora RPM/SRPM, locked vendored dependency, complete
  license notices, explicit tracked-source inventory, and reproducible metadata.
- Architecture-checked portable releases and Debian packages for x86_64,
  ARM64, and ARMv7, plus Fedora x86_64/ARM64 build paths. Distinguish native,
  emulated, and structural-only evidence in every release.

The audit evidence and measured limitations are recorded in [AUDIT.md](AUDIT.md).

## Next: 0.4 — repeatable release confidence

**Priority:** P1. **Ownership:** testing and release engineering.

1. Automate the current local gates in CI: formatting, Clippy, unit and PTY tests,
   static release validation, Fedora/Debian package builds, offline SRPM rebuild,
   and independent artifact comparison. Add native ARM64/ARMv7 runners so
   emulator smoke checks are not mistaken for native hardware validation.
   Keep publishing behind an explicit maintainer decision; repository secrets
   must not reach pull-request code. Add signing/provenance only after key
   ownership and CI trust policy are approved.
2. Expand namespace/permission testing into a disposable matrix: matching versus
   inherited proc mounts, own versus different UID, same-session SIGCONT,
   exited/reused identities, and restricted proc visibility. Destructive tests
   must verify the displayed owned-child identity before confirming any action.
3. Add deterministic synthetic stress workloads for large process inventories,
   CPU hotplug, repeated resize/cancel, device removal, and allocation retention.
   Publish workload generators and summaries, never real process captures.

**Exit evidence:** repeatable green runs on Fedora; no live-system side effects;
source RPM builds with networking disabled and an empty Cargo home; binary and
source license/privacy gates pass for every uploaded artifact.

## Then: 0.5 — richer process investigation

**Priority:** P2. **Ownership:** process and interface engineering.

- Add a process detail view using existing readable proc counters: parent PID,
  thread inventory, and I/O details. Do not expose environment variables or full
  command arguments by default.
- Evaluate a process tree and explicit multi-selection only after specifying
  selection stability, descendant races, cancellation, and permission behavior.
- Consider priority adjustment only with a documented reversible operation and
  kernel permission checks. Do not add a privileged helper merely to avoid a
  permission error.

**Exit evidence:** all details remain useful on narrow terminals; a refresh or
filter cannot retarget a pending action; regression journeys cover exit and
reparenting; large inventories stay within measured CPU/allocation budgets.

## Later: 0.6 — hardware and platform coverage

**Priority:** P2. **Ownership:** collector and packaging engineering.

- Expand Intel/Xe and AMD counter fixtures against pinned kernel ABI references.
  Investigate NVIDIA/NVML only as an optional adapter; keep the default build
  vendor-library-free and explicit about unavailable counters.
- Evaluate user-supplied sensor labels/scaling without interpreting shell code.
  Board voltage is currently the kernel's raw hwmon value, not a libsensors
  resistor-network calculation.
- Extend native ARM hardware coverage and evaluate another architecture such
  as riscv64 only after its toolchain, kernel interfaces, package metadata,
  and real/emulated evidence can be verified. Do not label ARMv7 as Fedora 45.

**Exit evidence:** documented counter units and fault states; no fabricated
fallback readings; target-specific binary/RPM smoke proof and license notices;
no regression in the existing x86_64 release.

## Ongoing budgets and release rules

- Keep the static executable below 1 MiB unless an explicitly justified feature
  changes that budget. Track native RPM payload and mandatory license-notice
  sizes separately; legal notices are not removable optimization overhead.
- Retain only current process/device counters, one reusable ranking vector, and
  current/previous terminal cells. Never accumulate samples or action history
  silently. Reclaim unusual peak allocations after inventories/viewports shrink.
- Compare performance using identical viewports, intervals, and deterministic
  workloads where possible. Live-host samples are smoke observations, not proof
  of a universal speed or memory ratio.
- A security fix takes precedence over a size win. Every process mutation needs
  a visible immutable target, explicit confirmation, and final kernel permission
  enforcement. New trust boundaries require a threat review and negative tests.
- A new dependency requires maintenance, license, advisory, and size review.
  Persistent history/export would need an opt-in retention/privacy design first.
