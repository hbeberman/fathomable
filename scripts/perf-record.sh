#!/usr/bin/env bash
# @okf-doc: /guide.md
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: perf-record.sh [--bin <binary-name>] -- <binary-args>

Build an optimized, frame-pointer-enabled binary and profile it interactively
with Linux perf until the binary exits. Artifacts are written under Cargo's
target directory in perf/<timestamp>-<pid>/ and printed on exit.

Examples:
  scripts/perf-record.sh --bin <binary-name> -- <binary-args>
  scripts/perf-record.sh --bin fathomable -- README.md

The just recipe forwards the path and optional binary name:
  just perf README.md
  just perf README.md fathomable

Pass binary arguments directly to this script after `--`. The profiled
program keeps the terminal, so use its normal quit action to finish the run.
The exact profiled executable is preserved beside perf.data for later analysis.

The helper discovers workspace binary targets; pass `--bin` when there is more
than one.
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

bin_name=""
binary_args=()
tmp_files=()
out_dir=""
perf_data=""
terminal_log=""
build_log=""
profiled_executable=""
perf_script=""
perf_report=""
folded=""
flamegraph=""

print_outputs() {
    local status=$?

    if [[ -n ${out_dir:-} ]]; then
        printf '\nPerf artifacts:\n'
        printf '  directory: %s\n' "$out_dir"
        for path in "$profiled_executable" "$perf_data" "$terminal_log" "$build_log" "$perf_report" "$perf_script" "$folded" "$flamegraph"; do
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
            fail "unexpected argument before --: $1 (pass binary arguments after --)"
    esac
done

[[ $(uname -s) == "Linux" ]] || fail "perf profiling is Linux-only"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required to parse Cargo metadata"
command -v perf >/dev/null 2>&1 || fail "perf is required; install your distro's perf/linux-tools package"
command -v script >/dev/null 2>&1 || fail "script is required; install util-linux"
[[ -t 0 && -t 1 ]] || fail "interactive profiling requires a terminal on stdin and stdout"

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
    fail "no workspace binary targets found in Cargo metadata"
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
umask 077
mkdir -p "$target_dir/perf"
mkdir -m 700 "$out_dir"
profile_target_dir="$target_dir/perf-build"

perf_data="$out_dir/perf.data"
terminal_log="$out_dir/terminal.log"
build_log="$out_dir/cargo-build.jsonl"
profiled_executable="$out_dir/$selected_bin"
perf_script="$out_dir/perf.script"
perf_report="$out_dir/perf-report.txt"
folded="$out_dir/perf.folded"
flamegraph="$out_dir/flamegraph.svg"

build_profile_binary() {
    local flags

    if [[ -v CARGO_ENCODED_RUSTFLAGS ]]; then
        flags=$CARGO_ENCODED_RUSTFLAGS
        [[ -z $flags ]] || flags+=$'\x1f'
        flags+='-Cforce-frame-pointers=yes'
        CARGO_TARGET_DIR="$profile_target_dir" CARGO_ENCODED_RUSTFLAGS="$flags" \
            cargo build --release --bin "$selected_bin" \
            --message-format=json-render-diagnostics
    else
        flags=${RUSTFLAGS-}
        [[ -z $flags ]] || flags+=" "
        flags+='-Cforce-frame-pointers=yes'
        CARGO_TARGET_DIR="$profile_target_dir" RUSTFLAGS="$flags" \
            cargo build --release --bin "$selected_bin" \
            --message-format=json-render-diagnostics
    fi
}

printf 'Building optimized frame-pointer binary `%s`...\n' "$selected_bin"
if ! build_profile_binary >"$build_log"; then
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
cp -- "$executable" "$profiled_executable"
[[ -x $profiled_executable ]] || fail "preserved executable is not runnable: $profiled_executable"

printf 'Recording with perf until the program exits: %s\n' "$profiled_executable"
if [[ ${#binary_args[@]} -gt 0 ]]; then
    printf 'Binary args:'
    printf ' %q' "${binary_args[@]}"
    printf '\n'
fi
printf 'Quit the program normally to finalize the perf data.\n'

set +e
script -q -e -f -O "$terminal_log" -- \
    perf record -F 997 --call-graph fp -o "$perf_data" -- \
    "$profiled_executable" "${binary_args[@]}"
record_status=$?
set -e

if [[ $record_status -ne 0 ]]; then
    printf 'perf-record: profiled command exited with code %d; see %s\n' \
        "$record_status" "$terminal_log" >&2
    if grep -qiE 'permission|not permitted|access|paranoid|Operation not permitted' \
        "$terminal_log" 2>/dev/null; then
        print_perf_hints
    fi
fi

if [[ -s $perf_data ]]; then
    if ! perf report --stdio -i "$perf_data" >"$perf_report"; then
        printf 'perf-record: warning: could not generate %s.\n' "$perf_report" >&2
        rm -f "$perf_report"
    fi
    if ! perf script -i "$perf_data" >"$perf_script"; then
        printf 'perf-record: warning: could not generate %s.\n' "$perf_script" >&2
        rm -f "$perf_script"
    fi
    if [[ -s $perf_script ]] \
        && command -v inferno-collapse-perf >/dev/null 2>&1 \
        && command -v inferno-flamegraph >/dev/null 2>&1; then
        if inferno-collapse-perf <"$perf_script" >"$folded" \
            && inferno-flamegraph <"$folded" >"$flamegraph"; then
            :
        else
            printf 'perf-record: warning: could not collapse perf data into folded stacks.\n' >&2
            rm -f "$folded" "$flamegraph"
        fi
    elif [[ -s $perf_data ]]; then
        printf 'perf-record: inferno tools not found; skipped folded stacks and SVG flamegraph.\n' >&2
        printf 'perf-record: optional tools: cargo install inferno --locked\n' >&2
    fi
elif [[ $record_status -eq 0 ]]; then
    printf 'perf-record: no perf.data was produced.\n' >&2
fi

if [[ $record_status -ne 0 ]]; then
    exit "$record_status"
fi
