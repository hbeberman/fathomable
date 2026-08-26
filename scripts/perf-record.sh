#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: perf-record.sh [seconds] [--bin <binary-name>] [-- <binary-args>]

Build a release binary and profile it with Linux perf for the requested number
of seconds. Artifacts are written under Cargo's target directory in
perf/<timestamp>-<pid>/ and printed on exit.

Examples:
  scripts/perf-record.sh 15 --bin <binary-name>
  scripts/perf-record.sh 30 --bin <binary-name> -- <args>

From Make:
  PERF_SECONDS=15 PERF_BIN=<binary-name> make perf

The just compatibility wrapper reads the same environment:
  PERF_SECONDS=15 PERF_BIN=<binary-name> just perf

Pass binary arguments directly to this script after `--`.

The generated bootstrap repo is library-only by default. Add a binary target
before running this helper.
USAGE
}

fail() {
    printf 'perf-record: error: %s\n' "$*" >&2
    exit 1
}

print_perf_hints() {
    cat >&2 <<'HINTS'

perf permission hints:
  - Check /proc/sys/kernel/perf_event_paranoid.
  - Use your distro's perf/linux-tools package matching the running kernel.
  - In locked-down containers or WSL2, perf may be unavailable even when the
    perf command exists.
  - Granting CAP_PERFMON, lowering kernel.perf_event_paranoid, or using sudo are
    operator decisions; this script does not make those changes.
HINTS
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo_root"

duration=15
duration_seen=0
bin_name=""
binary_args=()
tmp_files=()
out_dir=""
perf_data=""
stdout_log=""
stderr_log=""
build_log=""
folded=""
flamegraph=""
child_status=""

print_outputs() {
    local status=$?

    if [[ -n ${out_dir:-} ]]; then
        printf '\nPerf artifacts:\n'
        printf '  directory: %s\n' "$out_dir"
        for path in "$perf_data" "$stdout_log" "$stderr_log" "$build_log" "$folded" "$flamegraph"; do
            if [[ -n ${path:-} && -e $path ]]; then
                printf '  %s\n' "$path"
            fi
        done
    fi

    if [[ ${#tmp_files[@]} -gt 0 ]]; then
        rm -f "${tmp_files[@]}"
    fi

    exit "$status"
}
trap print_outputs EXIT

while (($#)); do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --bin)
            [[ $# -ge 2 ]] || fail "--bin requires a value"
            bin_name=$2
            shift 2
            ;;
        --)
            shift
            binary_args=("$@")
            break
            ;;
        --*)
            fail "unknown option before --: $1"
            ;;
        *)
            [[ $duration_seen -eq 0 ]] || fail "unexpected argument before --: $1"
            duration=$1
            duration_seen=1
            shift
            ;;
    esac
done

if ! [[ $duration =~ ^[1-9][0-9]*$ ]]; then
    fail "seconds must be a positive integer: $duration"
fi

[[ $(uname -s) == "Linux" ]] || fail "perf profiling is Linux-only"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required to parse Cargo metadata"
command -v perf >/dev/null 2>&1 || fail "perf is required; install your distro's perf/linux-tools package"
command -v timeout >/dev/null 2>&1 || fail "timeout is required; install GNU coreutils"

if [[ -r /proc/sys/kernel/perf_event_paranoid ]]; then
    paranoid=$(cat /proc/sys/kernel/perf_event_paranoid)
    if [[ $paranoid =~ ^-?[0-9]+$ && $paranoid -gt 2 ]]; then
        printf 'perf-record: warning: kernel.perf_event_paranoid=%s may block profiling.\n' "$paranoid" >&2
    fi
fi

metadata_file=$(mktemp)
tmp_files+=("$metadata_file")
cargo metadata --format-version 1 --no-deps >"$metadata_file"

target_dir=$(python3 - "$metadata_file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as fh:
    metadata = json.load(fh)

print(metadata["target_directory"])
PY
)

mapfile -t bin_targets < <(python3 - "$metadata_file" "$bin_name" <<'PY'
import json
import sys

metadata_path = sys.argv[1]
requested = sys.argv[2]

with open(metadata_path, encoding="utf-8") as fh:
    metadata = json.load(fh)

workspace = set(metadata.get("workspace_members", []))
bins = []
for package in metadata.get("packages", []):
    if package.get("id") not in workspace:
        continue
    for target in package.get("targets", []):
        if "bin" in target.get("kind", []):
            name = target.get("name", "")
            if not requested or name == requested:
                bins.append((name, package.get("name", "")))

for name, package_name in sorted(set(bins)):
    print(f"{name}\t{package_name}")
PY
)

if [[ ${#bin_targets[@]} -eq 0 ]]; then
    if [[ -n $bin_name ]]; then
        fail "no binary target named '$bin_name' found"
    fi
    fail "no binary targets found; the bootstrap scaffold is library-only until you add src/main.rs or [[bin]]"
fi

if [[ -z $bin_name && ${#bin_targets[@]} -gt 1 ]]; then
    printf 'perf-record: multiple binary targets found:\n' >&2
    printf '  %s\n' "${bin_targets[@]}" >&2
    fail "pass --bin <binary-name>"
fi

if [[ -z $bin_name ]]; then
    selected_bin=${bin_targets[0]%%$'\t'*}
else
    selected_bin=$bin_name
fi

timestamp=$(date -u +%Y%m%dT%H%M%SZ)
out_dir="$target_dir/perf/$timestamp-$$"
mkdir -p "$out_dir"

perf_data="$out_dir/perf.data"
stdout_log="$out_dir/stdout.log"
stderr_log="$out_dir/stderr.log"
build_log="$out_dir/cargo-build.jsonl"
folded="$out_dir/perf.folded"
flamegraph="$out_dir/flamegraph.svg"

printf 'Building release binary `%s`...\n' "$selected_bin"
if ! cargo build --release --bin "$selected_bin" --message-format=json-render-diagnostics >"$build_log"; then
    fail "cargo build failed; see $build_log"
fi

executable=$(python3 - "$build_log" "$selected_bin" <<'PY'
import json
import sys

path = ""
with open(sys.argv[1], encoding="utf-8") as fh:
    for line in fh:
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target") or {}
        executable = message.get("executable")
        if executable and target.get("name") == sys.argv[2]:
            path = executable

print(path)
PY
)

[[ -n $executable ]] || fail "could not locate release executable for '$selected_bin'; see $build_log"
[[ -x $executable ]] || fail "release executable is not runnable: $executable"

printf 'Recording %ss with perf: %s\n' "$duration" "$executable"
if [[ ${#binary_args[@]} -gt 0 ]]; then
    printf 'Binary args:'
    printf ' %q' "${binary_args[@]}"
    printf '\n'
fi

child_status=$(mktemp)
tmp_files+=("$child_status")

set +e
perf record -F 997 --call-graph dwarf -o "$perf_data" -- \
    timeout --signal=TERM --kill-after=5s "$duration" \
    bash -c '
        status_file=$1
        shift
        child=""
        forward_term() {
            if [[ -n $child ]]; then
                command kill -TERM "$child" 2>/dev/null || true
            fi
        }
        trap forward_term TERM
        "$@" &
        child=$!
        while true; do
            wait "$child"
            status=$?
            if ! command kill -0 "$child" 2>/dev/null; then
                break
            fi
        done
        printf "%s\n" "$status" >"$status_file"
        exit "$status"
    ' bash "$child_status" "$executable" "${binary_args[@]}" \
    >"$stdout_log" 2>"$stderr_log"
record_status=$?
set -e

profiled_status=""
if [[ -s $child_status ]]; then
    read -r profiled_status <"$child_status"
fi

if [[ $record_status -eq 124 && $profiled_status == 124 ]]; then
    printf 'perf-record: profiled binary exited with code 124 before the timeout\n' >&2
    exit 124
fi

timed_out=0
forced_kill=0
if [[ $record_status -eq 124 ]]; then
    timed_out=1
elif [[ $record_status -eq 137 && -z $profiled_status ]]; then
    timed_out=1
    forced_kill=1
elif [[ $record_status -ne 0 ]]; then
    printf 'perf-record: perf run failed with exit code %d; stderr follows:\n' "$record_status" >&2
    cat "$stderr_log" >&2
    if grep -qiE 'permission|not permitted|access|paranoid|Operation not permitted' "$stderr_log"; then
        print_perf_hints
    fi
    exit "$record_status"
fi

if [[ $timed_out -eq 1 && $forced_kill -eq 0 ]]; then
    printf 'Duration elapsed; timeout stopped the profiled binary with SIGTERM.\n'
elif [[ $timed_out -eq 1 ]]; then
    printf 'Duration elapsed; timeout force-killed the profiled binary after 5s.\n'
fi

if [[ -s $perf_data ]] \
    && command -v inferno-collapse-perf >/dev/null 2>&1 \
    && command -v inferno-flamegraph >/dev/null 2>&1; then
    if perf script -i "$perf_data" | inferno-collapse-perf >"$folded"; then
        inferno-flamegraph <"$folded" >"$flamegraph"
    else
        printf 'perf-record: warning: could not collapse perf data into folded stacks.\n' >&2
        rm -f "$folded" "$flamegraph"
    fi
elif [[ -s $perf_data ]]; then
    printf 'perf-record: inferno tools not found; skipped folded stacks and SVG flamegraph.\n' >&2
    printf 'perf-record: optional tools: cargo install inferno --locked\n' >&2
fi
