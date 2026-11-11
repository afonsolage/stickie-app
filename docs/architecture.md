# Stickie architecture

Stickie is a Windows-first, always-on-top overlay for note taking while gaming.
It is deliberately a **desktop overlay**, not an injected graphics hook: a
transparent borderless Bevy window is composited over the game by DWM. This
works for Windows 10/11 games running in windowed or borderless-windowed mode.
Covering exclusive-fullscreen games requires an injected graphics hook
(D3D/Vulkan swapchain interception) and is explicitly out of scope for the
first milestones. See [README](../README.md#known-limitations).

## Runtime shape

```text
Win+Shift+Tab ──(RegisterHotKey)──> global-hotkey window proc
                                          │ WakeUp event
                                          ▼
                                  Bevy winit event loop
                                          │
                          Update: OverlayController
                     ┌────────────────────┴────────────────────┐
                     ▼                                         ▼
          hide overlay: low-power                    show overlay:
          reactive update mode                       focus + probe foreground game
                                                     load workspace / last folder
                                                              │
                                                              ▼
                                                    egui overlay (bevy_egui)
```

Design rules that keep the game unaffected:

1. The overlay window is hidden and Bevy is in winit's reactive low-power
   update mode except while the user has the overlay open.
2. No background game polling. The foreground game is probed once when the
   overlay is opened; while the overlay is closed the app does no work.
3. Bevy is built without audio, 3D, PBR, physics, or Bevy UI. Rendering is a
   single transparent camera + egui pipeline.
4. Global hotkey handling runs through a lightweight Win32 message-only window
   (`global-hotkey`), not an installed input hook.
5. Disk writes are debounced (notes/stickies) and screenshot capture runs off
   the UI thread.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Thin binary entry point. |
| `src/lib.rs` | Public crate surface and plugin wiring. |
| `src/app.rs` | Bevy app/plugins, overlay controller state machine, autosave. |
| `src/config.rs` | Hotkey parsing/normalisation and app defaults. |
| `src/hotkey.rs` | Hotkey spec type and platform binding. |
| `src/platform/mod.rs` | Cross-platform facade: game probe, capture, reveal, focus, hotkey. |
| `src/platform/windows.rs` | Win32 implementations (foreground process, monitor capture, etc.). |
| `src/platform/stub.rs` | Non-Windows development fallbacks. |
| `src/store/mod.rs` | App data paths, config persistence, workspace model/storage. |
| `src/store/workspace.rs` | Workspace manifest, folders, notes, stickies, screenshots. |
| `src/ui/mod.rs` | eframe-free egui overlay composition. |
| `src/ui/shell.rs` | Scrim, command surface, navigation rail, status line. |
| `src/ui/notepad.rs` | Markdown notepad tab. |
| `src/ui/stickies.rs` | Sticky notes board. |
| `src/ui/files.rs` | Workspace folder browser. |
| `src/ui/screenshot.rs` | Screenshot library + crop/draw editor. |
| `src/ui/settings.rs` | Settings tab (hotkey, opacity, data location). |

The final split may consolidate small modules; the important boundary is
**platform / store / UI / app controller**.

## Game identity and workspace layout

The foreground game is identified from the foreground window's process image:

```text
game_id = "exe:" + normalized exe stem   e.g. "exe:eldenring"
```

This is stable across launches and requires no Steam integration. The display
name starts from the foreground window title (trimmed) and falls back to a
readable exe name. Steam app-id based ids and manual per-game naming are a
follow-up (`steam:<appid>` remains a compatible future id format).

```text
%APPDATA%/Stickie/                   (ProjectDirs, overridable with STICKIE_DATA_DIR)
├── config.toml                      # hotkey, overlay opacity, last workspace
├── logs/
└── workspaces/
    └── exe-eldenring/
        ├── workspace.toml           # metadata, stickies, last opened folder
        ├── Notes/                   # arbitrary .md files
        ├── Screenshots/             # captured annotations (PNG)
        └── Files/                   # user folders (game saves, builds, ...)
```

The `Files/` root is a normal filesystem tree. `workspace.toml` stores the
relative path of the last open folder so reopening a game resumes there.
Nothing is uploaded; Stickie has no network code.

## Platform facade (contract)

```rust
pub struct GameContext {
    pub workspace_id: String,      // stable, sanitised
    pub display_name: String,
    pub executable: Option<String>,
    pub window_title: Option<String>,
    pub process_id: Option<u32>,
}

pub fn overlay_supported() -> bool;
pub fn active_game() -> Option<GameContext>;          // foreground probe
pub fn restore_game_focus(game: &GameContext);
pub fn capture_game(game: Option<&GameContext>) -> Result<image::RgbaImage>;
pub fn open_in_file_manager(path: &Path) -> Result<()>;
```

The Windows implementation uses `GetForegroundWindow`, `OpenProcess`,
`QueryFullProcessImageNameW`, and `xcap` (Windows Graphics Capture). The
non-Windows implementation returns `None`/unsupported so the UI can be
developed and tested on Linux.

## Store facade (contract)

`Store` owns `AppPaths`, `Config`, and the currently open `Workspace`.
Everything returns `anyhow::Result` and the UI treats IO errors as inline
notices instead of panics.

Key operations:

```rust
Store::bootstrap() -> Result<Store>;
store.open_workspace(&GameContext) -> Result<()>;   // get or create dirs + manifest
store.set_last_folder(rel_path) -> Result<()>;
store.list_notes() -> Result<Vec<NoteEntry>>;
store.create_note() -> Result<NoteEntry>;
store.read_note(&NoteEntry) -> Result<String>;
store.write_note(&NoteEntry, &str) -> Result<()>;
store.list_folder(rel_path) -> Result<Vec<FileEntry>>;
store.create_folder(rel_path, name) -> Result<()>;
store.stickies() -> &[StickyNote];
store.save_sticky(StickyNote) -> Result<()>;
store.delete_sticky(id) -> Result<()>;
store.save_screenshot(&RgbaImage) -> Result<PathBuf>;
store.config / store.save_config;
```

## Overlay state machine

```text
Hidden --hotkey--> Probing --> LoadingWorkspace --> Visible
Visible --hotkey/Esc--> Hiding --> Hidden
Visible --capture--> Capturing --> Visible (editor opened)
```

- Show: probe game, open workspace, refresh file tree, request focus, show.
- Hide: debounce-save editor/stickies, hide window, restore focus to game.
- The overlay never renders a frame while hidden. Winit reactive mode is
  configured with a very long wait; only hotkey/OS wakeups resume the loop.
- Escape hides the overlay. Text fields keep Escape for themselves if needed
  (the UI handles focus priority before hiding).

## Performance targets

| Metric | Target |
| --- | --- |
| Idle (overlay hidden) CPU | < 0.1% typical, zero steady polling |
| Hidden working set | < 150 MB (Bevy + wgpu baseline; measured in CI/manual) |
| Show/toggle latency | < 100 ms to focus + paint cached UI |
| First workspace open | < 250 ms warm cache |
| Hotkey registration | global OS hotkey, no input hooks |

Release builds use `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, and
the minimal Bevy feature set in `Cargo.toml`.

## Security / privacy

- No telemetry, network, or auto-update in the MVP.
- Paths are always joined under the selected workspace root; list operations
  reject `..` traversal.
- Global hotkey registration is per-user and does not require elevation.
- The app never injects into other processes. That also means anti-cheat
  compatibility is better than injected overlays, but exclusive-fullscreen
  games are not covered.
