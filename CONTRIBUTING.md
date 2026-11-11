# Contributing

Stickie is early and moving quickly. Small, focused pull requests are very
welcome.

## Before you start

- For anything larger than a bug fix, open an issue first so the approach can
  be agreed on before you spend time.
- Read [docs/architecture.md](docs/architecture.md). The platform/store/UI
  separation and the "do no work while hidden" rule are intentional.
- Windows 11 is the primary target. Linux builds are supported so the UI can be
  developed and reviewed without a Windows machine.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --release
```

On Linux, the overlay opens windowed and starts visible; global hotkeys and
screen capture are stubs there. Use `STICKIE_SHOW_ON_START=1` to force a visible
overlay.

For the real Windows overlay while developing in WSL, install `cargo-xwin` plus
a toolchain with the `x86_64-pc-windows-msvc` target and run `just c`. Dev runs
start visible; `STICKIE_SHOW_ON_START=0 just c` tests the hidden/hotkey path.

Optional local data directory for testing:

```bash
STICKIE_DATA_DIR=/tmp/stickie-dev cargo run --release
```

## Pull requests

- Keep commits scoped and explain the user-visible behavior.
- Add tests for store/path logic. Windows-only Win32 code must be kept behind
  `cfg(target_os = "windows")` and reviewed extra carefully: CI compiles and
  tests the Windows target, but it cannot exercise real game/DWM behavior.
- Update README/docs when behavior, hotkeys, storage layout or limitations
  change.
- Never add telemetry, network access, process injection, or background polling
  without an explicit design discussion.

## Code style

- `cargo fmt` and clippy with warnings denied are required.
- Prefer plain data and small modules over framework-like abstractions.
- User data must never panic the app; surface errors as inline UI notices.
- Keep the default Bevy feature set minimal when adding dependencies. Every
  added crate is paid for in binary size and compile time.
