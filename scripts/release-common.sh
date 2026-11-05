#!/usr/bin/env bash
# Shared release metadata and structural validation; sourced by release scripts.

release_error() {
    printf 'Release error: %s\n' "$*" >&2
    return 1
}

release_version() {
    local line section= version= count=0
    local version_pattern='^[[:space:]]*version[[:space:]]*=[[:space:]]*"((0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))"[[:space:]]*(#.*)?$'
    while IFS= read -r line || [[ -n "$line" ]]; do
        if [[ "$line" =~ ^[[:space:]]*\[ ]]; then
            section=$line
        elif [[ "$section" =~ ^[[:space:]]*\[package\][[:space:]]*(#.*)?$ && "$line" =~ ^[[:space:]]*version[[:space:]]*= ]]; then
            [[ "$line" =~ $version_pattern ]] || { release_error 'Cargo package version must be an explicit stable MAJOR.MINOR.PATCH.'; return 1; }
            version=${BASH_REMATCH[1]}
            count=$((count + 1))
        fi
    done < "$root/Cargo.toml"
    [[ "$count" == 1 ]] || { release_error 'Cargo.toml must have exactly one package version.'; return 1; }
    printf '%s\n' "$version"
}

release_arch() {
    local arch=${1:-$(uname -m)}
    case "$arch" in
        x86_64|amd64|x86-64) printf 'x86_64\n' ;;
        aarch64|arm64) printf 'aarch64\n' ;;
        armv7|armv7l|armv8l|armhf) printf 'armv7\n' ;;
        *) release_error 'Unsupported architecture; expected x86_64, aarch64 or armv7.' ;;
    esac
}

release_target() {
    local arch
    arch=$(release_arch "${1:-}") || return 1
    case "$arch" in
        x86_64) printf 'x86_64-unknown-linux-musl\n' ;;
        aarch64) printf 'aarch64-unknown-linux-musl\n' ;;
        armv7) printf 'armv7-unknown-linux-musleabihf\n' ;;
    esac
}

# A runner is one executable, never a shell command or a string to evaluate.
# 125 means a cross executable could not be run, not that it passed execution.
release_run() {
    local arch host
    arch=$(release_arch "$1") || return 1
    shift
    if [[ -n "${KTOP_RUNNER:-}" ]]; then
        command -v "$KTOP_RUNNER" >/dev/null 2>&1 || { release_error 'KTOP_RUNNER must name an available executable.'; return 1; }
        "$KTOP_RUNNER" "$@"
    else
        host=$(release_arch) || return 1
        if [[ "$arch" != "$host" ]]; then
            printf 'Release warning: structural-only validation for %s; set KTOP_RUNNER to execute and verify --version.\n' "$arch" >&2
            return 125
        fi
        "$@"
    fi
}

release_check_binary() {
    local binary=$1 expected=$2 linkage=${3:-static} arch header segments dynamic sections output tool status
    arch=$(release_arch "${4:-}") || return 1
    for tool in readelf strings grep; do
        command -v "$tool" >/dev/null 2>&1 || { release_error "$tool is required for binary preflight."; return 1; }
    done
    [[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] || { release_error 'The release binary must be a regular, executable, non-symlink file.'; return 1; }
    header=$(LC_ALL=C readelf -hW -- "$binary") || return 1
    [[ "$header" =~ Data:[[:space:]]+2.s[[:space:]]complement,[[:space:]]little[[:space:]]endian ]] || { release_error 'Expected a little-endian ELF binary.'; return 1; }
    case "$arch" in
        x86_64)
            [[ "$header" =~ Class:[[:space:]]+ELF64 && "$header" =~ Machine:[[:space:]]+Advanced[[:space:]]Micro[[:space:]]Devices[[:space:]]X86-64 ]] || { release_error 'Expected an x86_64 ELF64 binary.'; return 1; }
            ;;
        aarch64)
            [[ "$header" =~ Class:[[:space:]]+ELF64 && "$header" =~ Machine:[[:space:]]+AArch64 ]] || { release_error 'Expected an AArch64 ELF64 binary.'; return 1; }
            ;;
        armv7)
            [[ "$header" =~ Class:[[:space:]]+ELF32 && "$header" =~ Machine:[[:space:]]+ARM[[:space:]] && "$header" =~ Flags:.*Version5[[:space:]]EABI.*hard-float[[:space:]]ABI ]] || { release_error 'Expected an ARM ELF32 EABI5 hard-float binary.'; return 1; }
            ;;
    esac
    [[ "$header" =~ Type:[[:space:]]+(EXEC|DYN)[[:space:]] ]] || { release_error 'Expected an executable ELF binary.'; return 1; }
    segments=$(LC_ALL=C readelf -lW -- "$binary") || return 1
    dynamic=$(LC_ALL=C readelf -dW -- "$binary") || return 1
    sections=$(LC_ALL=C readelf -SW -- "$binary") || return 1
    [[ ! "$sections" =~ \.debug_ && ! "$sections" =~ \.symtab ]] || { release_error 'Expected a stripped release binary.'; return 1; }
    if [[ "$linkage" == static ]]; then
        [[ ! "$segments" =~ INTERP && ! "$dynamic" =~ \(NEEDED\) ]] || { release_error 'Expected a static binary without an ELF interpreter or shared-library dependencies.'; return 1; }
    elif [[ "$linkage" != native ]]; then
        release_error 'Unknown release linkage policy.'
        return 1
    fi
    # Do not print matching strings: rejected input may contain private paths.
    if LC_ALL=C strings -a -- "$binary" | LC_ALL=C grep -E '(/home/|/Users/|/root/|/run/user/|/tmp/|/var/tmp/)' >/dev/null; then
        release_error 'The release binary contains an unremapped private or temporary path.'
        return 1
    fi
    for output in "$root" "${HOME:-}"; do
        if [[ -n "$output" && "$output" != / && "$output" != /build && "$output" != /src/ktop ]] && LC_ALL=C strings -a -- "$binary" | LC_ALL=C grep -F -- "$output" >/dev/null; then
            release_error 'The release binary contains a local source or home path.'
            return 1
        fi
    done
    # Only execute --version after enforcing the architecture and linkage policy.
    if output=$(release_run "$arch" "$binary" --version); then
        :
    else
        status=$?
        if [[ "$status" == 125 && -z "${KTOP_RUNNER:-}" && "$arch" != "$(release_arch)" ]]; then
            return 0
        fi
        release_error 'Release binary --version failed.'
        return 1
    fi
    [[ "$output" == "ktop $expected" ]] || { release_error 'Cargo and release binary versions do not match.'; return 1; }
}
