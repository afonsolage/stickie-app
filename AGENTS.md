# AGENTS.md

Guidance for coding agents working in this repository.

## What this is

Stickie is a Windows-first, lightweight in-game overlay written in Rust with
Bevy (window/render loop) and egui (UI). It is a notes-first Steam Overlay
replacement. Read `docs/architecture.md` before changing architecture.

## Non-negotiable constraints

1. **Do no work while hidden.** No background game polling, timers, input
   hooks, or spinning update loop. The foreground game is probed when the
   overlay opens. Hidden = winit low-power mode.
2. **No process injection.** The overlay is a transparent topmost DWM window.
   Exclusive-fullscreen support is a future project, not a hack.
3. **Keep the dependency tree small.** Bevy must stay on the minimal feature
   set in `Cargo.toml` (no audio, 3D, PBR, physics or Bevy UI).
4. **User data is sacred.** All file IO errors become UI notices; no panics or
   silent data loss on malformed config/workspaces.
5. **Windows is the product, Linux is a dev target.** Keep non-Windows stubs
   compiling; never `cfg`-gate the whole crate on Windows.

## Commands

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
STICKIE_SHOW_ON_START=1 cargo run
just c   # cross-compile + launch the Windows client from WSL
```

Cargo registry access is normal on developer machines; in sandboxed
environments set `CARGO_HOME=$PWD/.cargo-home`.

## Layout

- `src/app.rs` — Bevy app, controller, capture lifecycle, hotkey wakeups.
- `src/platform/` — Win32 probe/capture/global-hotkey; Linux stubs.
- `src/store/` — config + per-game workspaces (notes, stickies, files,
  screenshots). No Bevy dependency.
- `src/ui/` — egui overlay shell and components.
- `docs/` — architecture and UX research.

## Verification

Before reporting done: `cargo fmt --all -- --check`, `cargo test`, and if you
touched rendering/UI, run the app under Xvfb (`xvfb-run -a cargo run`) and
inspect a screenshot. Do not edit generated `target/` files or commit local
caches.
