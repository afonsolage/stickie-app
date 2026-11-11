win_target := "x86_64-pc-windows-msvc"

alias c := client-win
alias bw := build-win

# List available recipes.
default:
    @just --list

# Validate formatting, lints and tests locally.
ci: fmt clippy test

# Check formatting.
fmt:
    cargo fmt --all -- --check

# Run clippy with warnings denied.
clippy:
    cargo clippy --all-targets -- -D warnings

# Run the test suite.
test:
    cargo test --all-targets

# Requires cargo-xwin and a Rust toolchain with the Windows MSVC target.
# STICKIE_WIN_TOOLCHAIN picks a toolchain; STICKIE_WIN_PROFILE=release builds
# optimised.
#
# Cross-compile the Windows client with cargo-xwin.
build-win:
    #!/usr/bin/env bash
    set -euo pipefail
    target="{{win_target}}"
    profile="${STICKIE_WIN_PROFILE:-debug}"

    if [[ -n "${STICKIE_WIN_TOOLCHAIN:-}" ]]; then
        toolchain="$STICKIE_WIN_TOOLCHAIN"
    else
        toolchain=""
        for candidate in nightly-2026-08-15 nightly stable; do
            if rustup target list --installed --toolchain "$candidate" 2>/dev/null \
                | grep -qx "$target"; then
                toolchain="$candidate"
                break
            fi
        done
    fi

    if [[ -z "$toolchain" ]]; then
        cat >&2 <<'EOF'
    error: no Rust toolchain with the x86_64-pc-windows-msvc target was found.

    Install one, for example:
      rustup toolchain install nightly-2026-08-15 --target x86_64-pc-windows-msvc

    Then install the Windows SDK/cross linker support:
      cargo install cargo-xwin

    Set STICKIE_WIN_TOOLCHAIN=<toolchain> to use a specific one.
    EOF
        exit 1
    fi

    if ! command -v cargo-xwin >/dev/null 2>&1; then
        echo "error: cargo-xwin is required for Windows cross-compilation." >&2
        echo "Install it with: cargo install cargo-xwin" >&2
        exit 1
    fi

    cargo_args=(--bin stickie)
    if [[ "$profile" == "release" ]]; then
        cargo_args+=(--release)
    fi

    echo "Building Stickie for Windows ($profile) with $toolchain..."
    cargo +"$toolchain" xwin build --target "$target" "${cargo_args[@]}"
    echo "Built target/$target/$profile/stickie.exe"

# Unlike the packaged Windows app, this dev recipe starts visible by default so
# you can see the overlay immediately. STICKIE_SHOW_ON_START=0 starts hidden.
#
# Build the Windows client and launch it from WSL through interop.
client-win: build-win
    #!/usr/bin/env bash
    set -euo pipefail
    target="{{win_target}}"
    profile="${STICKIE_WIN_PROFILE:-debug}"
    exe="target/$target/$profile/stickie.exe"

    if [[ ! -f "$exe" ]]; then
        echo "error: expected Windows binary at $exe" >&2
        exit 1
    fi
    if [[ ! -e /proc/sys/fs/binfmt_misc/WSLInterop ]]; then
        echo "error: Windows interop is not available; run $exe on Windows." >&2
        exit 1
    fi

    export RUST_LOG="${RUST_LOG:-info,stickie=debug}"
    export STICKIE_SHOW_ON_START="${STICKIE_SHOW_ON_START:-1}"
    # WSLENV forwards selected variables into Windows. WGPU_BACKEND is optional;
    # set it in WSL (e.g. WGPU_BACKEND=vulkan) if the default adapter misbehaves.
    export WSLENV="${WSLENV:+$WSLENV:}RUST_LOG:STICKIE_SHOW_ON_START:STICKIE_DATA_DIR:WGPU_BACKEND"

    echo "Launching $exe..."
    exec "./$exe"
