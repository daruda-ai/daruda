#!/usr/bin/env bash
# Shared Cargo gates for all platform jobs. OS setup and runtime checks stay
# in ci.yml; package coverage is defined here once.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [ "$#" -eq 0 ]; then
    echo "Usage: bash scripts/ci-cargo.sh {clippy|app-features|test|vendor-test|screenshot-test|doc} [cargo options]" >&2
    exit 2
fi
gate="$1"
shift

packages=(
    ghostty_vt ghostty_vt_sys daruda_terminal daruda daruda_config
    daruda_store daruda_agent daruda_update daruda_acp daruda_core
    daruda_flow daruda_project daruda_ui daruda_control_types daruda_content
    daruda_flow_edit daruda_transcript daruda_chat daruda_settings daruda_runlog vendor_gpui test_process strings_gen
)

case "$gate" in
    clippy) command=(cargo clippy --locked "$@"); packages+=(ferrum_flow) ;;
    test) command=(cargo test --locked "$@") ;;
    vendor-test)
        # Upstream suites plus daruda's patch tests. They read nothing outside
        # `vendor/`, the manifests, and the toolchain, so CI runs them only when
        # `check-packaging-changes.sh` selects the diff.
        command=(cargo test --locked "$@")
        packages=(ferrum_flow gpui-base gpui-component)
        ;;
    screenshot-test)
        # The feature adds a few dozen app tests; the rest are the ones `test`
        # already ran. Diffing the two lists keeps the selection free of any
        # naming convention a new feature-gated test would have to follow.
        plain="$(mktemp)"
        shot="$(mktemp)"
        trap 'rm -f "$plain" "$shot"' EXIT
        cargo test --locked "$@" -p daruda -- --list | sed -n 's/: test$//p' | sort >"$plain"
        cargo test --locked "$@" -p daruda --features screenshot -- --list |
            sed -n 's/: test$//p' | sort >"$shot"
        only=()
        while IFS= read -r name; do
            only+=("$name")
        done < <(comm -13 "$plain" "$shot")
        # An empty filter list would run the whole suite again.
        if [ "${#only[@]}" -eq 0 ]; then
            echo "screenshot-test: the feature added no tests; check the --list output" >&2
            exit 1
        fi
        cargo test --locked "$@" -p daruda --features screenshot -- --exact "${only[@]}"
        exit
        ;;
    app-features)
        exec cargo clippy --locked "$@" -p daruda --all-features --all-targets -- -D warnings
        ;;
    doc)
        command=(cargo doc --locked "$@" --no-deps)
        packages=(daruda_flow daruda_project daruda_ui daruda_control_types daruda_content
            daruda_flow_edit daruda_transcript daruda_chat daruda_settings daruda_runlog daruda_core daruda_update ghostty_vt_sys ghostty_vt daruda_agent)
        export RUSTDOCFLAGS="${RUSTDOCFLAGS:+$RUSTDOCFLAGS }-D warnings"
        ;;
    *) echo "Unknown Cargo gate: $gate" >&2; exit 2 ;;
esac

for package in "${packages[@]}"; do
    command+=(-p "$package")
done
if [ "$gate" = clippy ]; then
    command+=(--all-targets -- -D warnings)
fi
exec "${command[@]}"
