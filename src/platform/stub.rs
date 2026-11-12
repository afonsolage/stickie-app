//! Non-Windows development stubs.
//!
//! Stickie is a Windows product; Linux (and macOS) builds exist so the UI,
//! store and app controller can be developed and tested without a Windows
//! machine. The stubs below keep exactly the same public signatures as the
//! Windows implementation but return [`None`]/unsupported errors instead of
//! pretending to work.
//!
//! Opening files and folders is the exception: it is implemented with
//! `xdg-open` so the developer UI can exercise its "reveal in file manager"
//! flow.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use super::GameContext;

/// Builds a clear "not implemented on this platform" error.
fn unsupported(feature: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{feature} is only implemented on Windows; this {} build has no native backend",
        super::platform_name()
    )
}

/// Windows-only: probes the foreground game. Always [`None`] here.
pub fn active_game() -> Option<GameContext> {
    None
}

/// Windows-only: restores focus to the game window. No-op here.
pub fn restore_game_focus(_game: &GameContext) {}

/// Windows-only: selects the DirectComposition presentation path. No-op here.
pub fn configure_overlay_graphics() {}

/// Windows-only: returns the primary monitor size. Unavailable in the stub.
pub fn primary_monitor_size() -> Option<(u32, u32)> {
    None
}

/// Windows-only: captures the primary display. Always an error here.
pub fn capture_screen() -> Result<RgbaImage> {
    capture_game(None)
}

/// Windows-only: captures the game or primary monitor. Always an error here.
pub fn capture_game(_game: Option<&GameContext>) -> Result<RgbaImage> {
    Err(unsupported("screenshot capture"))
}

/// Opens `path` in the desktop file manager using `xdg-open`.
pub fn open_in_file_manager(path: &Path) -> Result<()> {
    open_with_xdg_open(path)
}

/// Reveals `path` in the desktop file manager (opens the containing folder).
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    };
    open_with_xdg_open(&target)
}

fn open_with_xdg_open(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("cannot open an empty path");
    }
    if !path.exists() {
        bail!("path does not exist: {}", path.display());
    }
    Command::new("xdg-open")
        .arg(path)
        .spawn()
        .with_context(|| format!("failed to launch xdg-open for {}", path.display()))?;
    Ok(())
}
