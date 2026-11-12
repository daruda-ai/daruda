#!/usr/bin/env bash
# Lint: app code reaches gh only through the `GhHost` global.
#
# Test builds install a scripted `FakeGh` in `GhHost`, so an app test
# spawns no `gh` process unless something bypasses that global. The rule
# covers all app code, production and tests alike: a production bypass is
# reachable from tests too.
#
# Rule: outside packages/app/src/gh_host.rs, no file under packages/app/src
# may name `SystemGh` or build a `gh` child (`command("gh")`, bare or
# path-qualified, `command_on_path("gh")`, `Command::new("gh")`). Line
# comments are ignored. A call split across lines or a name built at runtime
# is not caught; the `Gh` trait boundary is what enforces it.
# A file that genuinely needs one goes in ALLOWED with its reason.
#
# Usage:
#   scripts/lint-real-gh-tests.sh
#   scripts/lint-real-gh-tests.sh --self-test
#
# Exit codes:
#   0 — clean
#   1 — at least one bypass outside the allowlist
#   2 — missing source tree or argument failure

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# "path|reason" — paths relative to the repository root.
ALLOWED=()

HOST="packages/app/src/gh_host.rs"

# scan <root> [allowed entries...]
scan() {
    local root="$1"
    shift
    if [ ! -d "$root/packages/app/src" ]; then
        echo "[lint-real-gh-tests] missing $root/packages/app/src" >&2
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
        LINT_REAL_GH_SKIP="$(printf '%s\n' "$HOST" ${allowed_paths[@]+"${allowed_paths[@]}"})"
        export LINT_REAL_GH_SKIP
        find packages/app/src -type f -name '*.rs' -print0 \
            | xargs -0 perl -ne '
                BEGIN {
                    %skip = map { $_ => 1 } split /\n/, $ENV{LINT_REAL_GH_SKIP};
                }
                unless ($skip{$ARGV}) {
                    s{(^|\s)//.*}{$1};
                    print "$ARGV:$.: real gh outside GhHost: $_"
                        if /\bSystemGh\b
                            | \b(?:command|command_on_path)\s*\(\s*"gh"
                            | \bCommand::new\s*\(\s*"gh"/x;
                }
                close ARGV if eof;
            '
    )" || {
        echo "[lint-real-gh-tests] scanner failed under $root" >&2
        return 2
    }
    if [ -n "$violations" ]; then
        printf '%s\n' "$violations"
        return 1
    fi
}

self_test() {
    SELF_TEST_DIR="$(mktemp -d)"
    trap 'rm -rf "$SELF_TEST_DIR"' EXIT
    local temp="$SELF_TEST_DIR"
    mkdir -p "$temp/packages/app/src/nested"
    local violation="$temp/packages/app/src/nested/spawns.rs"

    printf '%s\n' 'let out = daruda_core::process::command("gh").output();' >"$violation"
    local status=0
    scan "$temp" >/dev/null || status=$?
    if [ "$status" -ne 1 ]; then
        echo "[lint-real-gh-tests] self-test: a gh spawn exited $status, not 1" >&2
        return 1
    fi
    if ! scan "$temp" "packages/app/src/nested/spawns.rs|self-test" >/dev/null; then
        echo "[lint-real-gh-tests] self-test: an allowlisted file failed" >&2
        return 1
    fi

    local spawn
    for spawn in \
        'use daruda_core::process::command; let out = command("gh").output();' \
        'let out = command( "gh" ).output();' \
        'let out = command_on_path ("gh").output();' \
        'let out = std::process::Command::new( "gh").output();'; do
        printf '%s\n' "$spawn" >"$violation"
        status=0
        scan "$temp" >/dev/null || status=$?
        if [ "$status" -ne 1 ]; then
            echo "[lint-real-gh-tests] self-test: \`$spawn\` exited $status, not 1" >&2
            return 1
        fi
    done

    printf '%s\n' 'let out = test_process::command("ls"); gh_command(dir);' >"$violation"
    if ! scan "$temp" >/dev/null; then
        echo "[lint-real-gh-tests] self-test: a non-gh spawn failed" >&2
        return 1
    fi

    printf '%s\n' 'GhHost::set_global(Arc::new(SystemGh), cx);' >"$violation"
    status=0
    scan "$temp" >/dev/null || status=$?
    if [ "$status" -ne 1 ]; then
        echo "[lint-real-gh-tests] self-test: a SystemGh install exited $status, not 1" >&2
        return 1
    fi

    printf '%s\n' '// Comments may name SystemGh and Command::new("gh").' \
        'let x = 1; /// as may trailing ones: command("gh")' >"$violation"
    printf '%s\n' 'Arc::new(daruda_github::SystemGh)' >"$temp/$HOST"
    if ! scan "$temp" >/dev/null; then
        echo "[lint-real-gh-tests] self-test: gh_host.rs or a comment failed" >&2
        return 1
    fi
    echo "[lint-real-gh-tests] self-test passed"
}

case "${1-}" in
    "")
        if scan "$ROOT" ${ALLOWED[@]+"${ALLOWED[@]}"}; then
            echo "[lint-real-gh-tests] ok"
        else
            status=$?
            if [ "$status" -eq 1 ]; then
                echo
                echo "Reach gh through crate::gh_host::GhHost; tests get FakeGh."
                echo "If real gh is unavoidable, add \"path|reason\" to ALLOWED."
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
