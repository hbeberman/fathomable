#!/usr/bin/env bash
set -euo pipefail

msg_file=${1:-}
if [[ -z $msg_file || ! -f $msg_file ]]; then
    echo "commit-msg: expected a commit message file argument" >&2
    exit 1
fi

# The message path is resolved up front and nothing below reads the working
# directory, so this linter must not depend on being run from inside the
# repository. Git supplies an in-repository working directory for the hook.
msg_file=$(cd "$(dirname -- "$msg_file")" && pwd -P)/$(basename -- "$msg_file")

if grep -Eiq '^[[:space:]]*(Claude-Session:|Co-Authored-By:[^[:cntrl:]]*(Copilot|Claude|Codex|noreply@openai[.]com))' "$msg_file"; then
    echo "✗ commit message contains forbidden Copilot, Claude, Codex, or Claude-Session metadata" >&2
    exit 1
fi

header=""
case "$(basename -- "$msg_file")" in
    MERGE_MSG|SQUASH_MSG)
        ;;
    *)
        while IFS= read -r line || [[ -n $line ]]; do
            case "$line" in
                ''|'#'*) continue ;;
            esac
            header=$line
            break
        done <"$msg_file"

        if [[ -z $header ]]; then
            echo "✗ commit message does not contain a Conventional Commits header" >&2
            exit 1
        fi

        case "$header" in
            "Merge "*|"Revert "*|"fixup! "*|"squash! "*|"amend! "*)
                ;;
            *)
                types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
                header_re="^(${types})(\([a-z0-9_-]+\))?!?: .+"

                if ! [[ $header =~ $header_re ]]; then
                    cat >&2 <<MSG
✗ commit message does not follow Conventional Commits 1.0.0

   header: $header

Expected format:
   <type>[optional scope][!]: <description>

   type   one of: feat, fix, docs, style, refactor, perf, test, build, ci,
                  chore, revert
   scope  optional, lowercase [a-z0-9_-]+
   !      optional, signals a breaking change
   header up to 72 characters
MSG
                    exit 1
                fi

                if (( ${#header} > 72 )); then
                    printf '✗ commit header exceeds 72 characters (%d)\n   %s\n' \
                        "${#header}" "$header" >&2
                    exit 1
                fi
                ;;
        esac
        ;;
esac
