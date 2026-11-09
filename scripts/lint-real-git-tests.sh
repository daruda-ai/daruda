#!/usr/bin/env bash
# Lint: app code reaches git only through the `GitHost` global.
#
# Test builds install a scripted `FakeGit` in `GitHost`, so an app test
# spawns no `git` process unless something bypasses that global. Each
# bypass below spawned real git from tests: a storm of them hung macOS.
#
# Rule: outside packages/app/src/git_host.rs, no file under packages/app/src
# may name `SystemGit` or build a `git` child (`command("git")`, bare or
# path-qualified, `command_on_path("git")`, `Command::new("git")`). Line
# comments are ignored. A call split across lines or a name built at runtime
# is not caught; the `pub(crate)` git boundary is what enforces it.
# A file that genuinely needs one goes in ALLOWED with its reason.
#
# Usage:
#   scripts/lint-real-git-tests.sh
#   scripts/lint-real-git-tests.sh --self-test
#
# Exit codes:
#   0 — clean
#   1 — at least one bypass outside the allowlist
#   2 — missing source tree or argument failure

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# "path|reason" — paths relative to the repository root.
ALLOWED=()

HOST="packages/app/src/git_host.rs"

# scan <root> [allowed entries...]
scan() {
    local root="$1"
    shift
    if [ ! -d "$root/packages/app/src" ]; then
        echo "[lint-real-git-tests] missing $root/packages/app/src" >&2
        return 2
    fi
    local allowed_paths=()
    local entry
    for entry in "$@"; do
        allowed_paths+=("${entry%%|*}")
    done
    local violations
    violations="$(
        cd "$root"
        LINT_REAL_GIT_SKIP="$(printf '%s\n' "$HOST" ${allowed_paths[@]+"${allowed_paths[@]}"})"
        export LINT_REAL_GIT_SKIP
        find packages/app/src -type f -name '*.rs' -print0 \
            | xargs -0 perl -ne '
                BEGIN {
                    %skip = map { $_ => 1 } split /\n/, $ENV{LINT_REAL_GIT_SKIP};
                }
                unless ($skip{$ARGV}) {
                    s{(^|\s)//.*}{$1};
                    print "$ARGV:$.: real git outside GitHost: $_"
                        if /\bSystemGit\b
                            | \b(?:command|command_on_path)\s*\(\s*"git"
                            | \bCommand::new\s*\(\s*"git"/x;
                }
                close ARGV if eof;
            '
    )" || {
        echo "[lint-real-git-tests] scanner failed under $root" >&2
        return 2
    }
    if [ -n "$violations" ]; then
        printf '%s\n' "$violations"
        return 1
    fi
}

self_test() {
    local temp
    temp="$(mktemp -d)"
    trap 'rm -rf "$temp"' RETURN
    mkdir -p "$temp/packages/app/src/nested"
    local violation="$temp/packages/app/src/nested/spawns.rs"

    printf '%s\n' 'let out = daruda_core::process::command("git").output();' >"$violation"
    local status=0
    scan "$temp" >/dev/null || status=$?
    if [ "$status" -ne 1 ]; then
        echo "[lint-real-git-tests] self-test: a git spawn exited $status, not 1" >&2
        return 1
    fi
    if ! scan "$temp" "packages/app/src/nested/spawns.rs|self-test" >/dev/null; then
        echo "[lint-real-git-tests] self-test: an allowlisted file failed" >&2
        return 1
    fi

    local spawn
    for spawn in \
        'use daruda_core::process::command; let out = command("git").output();' \
        'let out = command( "git" ).output();' \
        'let out = command_on_path ("git").output();' \
        'let out = std::process::Command::new( "git").output();'; do
        printf '%s\n' "$spawn" >"$violation"
        status=0
        scan "$temp" >/dev/null || status=$?
        if [ "$status" -ne 1 ]; then
            echo "[lint-real-git-tests] self-test: \`$spawn\` exited $status, not 1" >&2
            return 1
        fi
    done

    printf '%s\n' 'let out = test_process::command("ls"); git_command(dir);' >"$violation"
    if ! scan "$temp" >/dev/null; then
        echo "[lint-real-git-tests] self-test: a non-git spawn failed" >&2
        return 1
    fi

    printf '%s\n' 'GitHost::set_global(Arc::new(SystemGit), cx);' >"$violation"
    status=0
    scan "$temp" >/dev/null || status=$?
    if [ "$status" -ne 1 ]; then
        echo "[lint-real-git-tests] self-test: a SystemGit install exited $status, not 1" >&2
        return 1
    fi

    printf '%s\n' '// Comments may name SystemGit and Command::new("git").' \
        'let x = 1; /// as may trailing ones: command("git")' >"$violation"
    printf '%s\n' 'Arc::new(crate::lane::git::SystemGit)' >"$temp/$HOST"
    if ! scan "$temp" >/dev/null; then
        echo "[lint-real-git-tests] self-test: git_host.rs or a comment failed" >&2
        return 1
    fi
    echo "[lint-real-git-tests] self-test passed"
}

case "${1-}" in
    "")
        if scan "$ROOT" ${ALLOWED[@]+"${ALLOWED[@]}"}; then
            echo "[lint-real-git-tests] ok"
        else
            status=$?
            if [ "$status" -eq 1 ]; then
                echo
                echo "Reach git through crate::git_host::GitHost; tests get FakeGit."
                echo "If real git is unavoidable, add \"path|reason\" to ALLOWED."
            fi
            exit "$status"
        fi
        ;;
    --self-test)
        self_test
        ;;
    *)
        echo "usage: $0 [--self-test]" >&2
        exit 2
        ;;
esac
