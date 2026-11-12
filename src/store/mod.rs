//! Persistent storage for Stickie.
//!
//! The store owns the app data paths, the TOML configuration file, and the
//! currently open [`Workspace`] (notes, sticky notes, file browser, and
//! screenshots). It is deliberately free of Bevy and platform dependencies so
//! it can be unit tested without a window; the platform and UI layers only see
//! plain Rust types.
//!
//! Layout below the app data root (see [`AppPaths`]):
//!
//! ```text
//! <root>/
//! ├── config.toml
//! ├── logs/
//! └── workspaces/
//!     └── exe-eldenring/
//!         ├── workspace.toml
//!         ├── Notes/
//!         ├── Screenshots/
//!         └── Files/
//! ```
//!
//! The app data root is `%APPDATA%/Stickie` on Windows (via `directories`) and
//! can be overridden with the `STICKIE_DATA_DIR` environment variable.

#[cfg(test)]
mod tests;
mod workspace;

pub use workspace::{
    FileEntry, NoteEntry, PINNED_STICKY_MAX_HEIGHT, PINNED_STICKY_MAX_WIDTH,
    PINNED_STICKY_MIN_HEIGHT, PINNED_STICKY_MIN_WIDTH, PinnedSticky, StickyColor, StickyNote,
    Workspace, WorkspaceDescriptor, sanitize_workspace_id,
};

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use image::RgbaImage;
use serde::{Deserialize, Serialize};

/// Environment variable that overrides the app data directory.
pub const DATA_DIR_ENV: &str = "STICKIE_DATA_DIR";
/// Default global overlay toggle hotkey.
pub const DEFAULT_TOGGLE_HOTKEY: &str = "Win+Shift+Tab";
/// Default scrim opacity of the overlay.
pub const DEFAULT_SCRIM_OPACITY: f32 = 0.45;
/// Maximum accepted scrim opacity.
pub const MAX_SCRIM_OPACITY: f32 = 0.85;
/// Name of the configuration file inside the app data root.
const CONFIG_FILE: &str = "config.toml";
/// Name of the directory that holds workspaces.
const WORKSPACES_DIR: &str = "workspaces";
/// Name of the directory that holds logs.
const LOGS_DIR: &str = "logs";

/// Filesystem locations used by Stickie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// Discovers the app data root.
    ///
    /// `STICKIE_DATA_DIR` overrides the location when set to a non-empty value;
    /// otherwise the platform project directory is used (`%APPDATA%/Stickie` on
    /// Windows).
    pub fn discover() -> Result<Self> {
        if let Some(value) = env::var_os(DATA_DIR_ENV) {
            let value = value.to_string_lossy();
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Ok(Self::at(PathBuf::from(trimmed)));
            }
        }
        // `from_path` (rather than `from`) keeps the documented layout of
        // `%APPDATA%/Stickie` on Windows instead of nesting an organisation
        // folder below it.
        let project_dirs = ProjectDirs::from_path(PathBuf::from("Stickie"))
            .context("could not determine the Stickie app data directory")?;
        Ok(Self::at(project_dirs.data_dir().to_path_buf()))
    }

    /// Creates paths rooted at an explicit directory (used by tests and portable
    /// installs).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root of all Stickie data.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the TOML configuration file.
    pub fn config_file(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// Directory that holds one workspace per game.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join(WORKSPACES_DIR)
    }

    /// Directory that holds application logs.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join(LOGS_DIR)
    }

    /// Directory of a single workspace, using its sanitised id.
    pub fn workspace_dir(&self, workspace_id: &str) -> PathBuf {
        self.workspaces_dir()
            .join(sanitize_workspace_id(workspace_id))
    }

    /// Creates the app data directories if they do not exist yet.
    fn ensure_layout(&self) -> Result<()> {
        for directory in [&self.root, &self.workspaces_dir(), &self.logs_dir()] {
            fs::create_dir_all(directory).with_context(|| {
                format!(
                    "failed to create app data directory {}",
                    directory.display()
                )
            })?;
        }
        Ok(())
    }
}

/// User configuration persisted in `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Global hotkey that toggles the overlay, in the platform hotkey syntax.
    pub toggle_hotkey: String,
    /// Scrim opacity, clamped to `0.0..=MAX_SCRIM_OPACITY` when loaded or saved.
    pub scrim_opacity: f32,
    /// Whether the overlay starts hidden on launch.
    pub start_hidden: bool,
    /// Sanitised id of the last opened workspace, if any.
    pub last_workspace: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            toggle_hotkey: DEFAULT_TOGGLE_HOTKEY.to_string(),
            scrim_opacity: DEFAULT_SCRIM_OPACITY,
            start_hidden: true,
            last_workspace: None,
        }
    }
}

impl Config {
    /// Loads the configuration, falling back to defaults when the file is
    /// missing or corrupt.
    fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(contents) => match toml::from_str::<Self>(&contents) {
                Ok(config) => Ok(config.normalized()),
                Err(error) => {
                    log::warn!(
                        "config file {} is malformed ({error}); backing it up and using defaults",
                        path.display()
                    );
                    if let Err(backup_error) = workspace::backup_file(path) {
                        log::warn!("could not back up malformed config file: {backup_error:#}");
                    }
                    Ok(Self::default())
                }
            },
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to read config file {}", path.display()))
            }
        }
    }

    /// Saves the configuration to `path` atomically.
    fn save(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string_pretty(self).context("failed to serialize config")?;
        workspace::write_atomic(path, contents.as_bytes())
    }

    /// Validates and clamps user supplied values.
    fn normalized(mut self) -> Self {
        self.toggle_hotkey = non_empty_or_default(&self.toggle_hotkey, DEFAULT_TOGGLE_HOTKEY);
        self.scrim_opacity = normalize_scrim_opacity(self.scrim_opacity);
        self.last_workspace = self.last_workspace.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(sanitize_workspace_id(trimmed))
            }
        });
        self
    }
}

/// Clamps the scrim opacity, replacing non-finite values with the default.
fn normalize_scrim_opacity(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_SCRIM_OPACITY)
    } else {
        DEFAULT_SCRIM_OPACITY
    }
}

/// Trims a value, returning `fallback` when the result is empty.
fn non_empty_or_default(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Facade over the app data paths, configuration, and the open workspace.
///
/// The UI treats every returned [`anyhow::Error`] as an inline notice; the store
/// itself never panics on user data.
pub struct Store {
    paths: AppPaths,
    config: Config,
    workspace: Option<Workspace>,
}

impl Store {
    /// Bootstraps the store in the discovered app data directory.
    pub fn bootstrap() -> Result<Self> {
        Self::bootstrap_with_paths(AppPaths::discover()?)
    }

    /// Bootstraps the store in an explicit app data directory (tests/portable).
    pub fn bootstrap_at(root: impl Into<PathBuf>) -> Result<Self> {
        Self::bootstrap_with_paths(AppPaths::at(root))
    }

    fn bootstrap_with_paths(paths: AppPaths) -> Result<Self> {
        paths.ensure_layout()?;
        let config_file = paths.config_file();
        let config = Config::load(&config_file)?;
        let mut store = Self {
            paths,
            config,
            workspace: None,
        };
        if !store.paths.config_file().is_file() {
            let config = store.config.clone();
            store.save_config(config)?;
        }
        log::info!("store rooted at {}", store.paths.root().display());
        Ok(store)
    }

    /// The app data paths used by this store.
    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// The current configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Replaces and persists the configuration.
    ///
    /// Values are normalised first: the hotkey falls back to the default when
    /// empty, the scrim opacity is clamped to `0.0..=MAX_SCRIM_OPACITY`, and the
    /// last workspace id is sanitised.
    pub fn save_config(&mut self, config: Config) -> Result<()> {
        let config = config.normalized();
        config.save(&self.paths.config_file())?;
        self.config = config;
        Ok(())
    }

    /// The currently open workspace, if any.
    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    /// Opens (or creates) the workspace described by `descriptor`.
    ///
    /// The workspace directory is derived from the sanitised workspace id. The
    /// manifest is read when present, its game metadata is refreshed from the
    /// descriptor, and the result is persisted durably. Opening a workspace also
    /// remembers it as `config.last_workspace`.
    pub fn open_workspace(&mut self, descriptor: WorkspaceDescriptor<'_>) -> Result<&Workspace> {
        let workspace_id = sanitize_workspace_id(descriptor.workspace_id);
        let workspace_root = self.paths.workspaces_dir().join(&workspace_id);
        let workspace = Workspace::open(workspace_root, descriptor)?;
        if self.config.last_workspace.as_deref() != Some(workspace_id.as_str()) {
            let mut config = self.config.clone();
            config.last_workspace = Some(workspace_id);
            config
                .save(&self.paths.config_file())
                .context("failed to remember the last opened workspace")?;
            self.config = config;
        }
        self.workspace = Some(workspace);
        self.workspace.as_ref().context("workspace is not open")
    }

    /// Directory that stores markdown notes for the open workspace.
    pub fn notes_dir(&self) -> Option<&Path> {
        self.workspace.as_ref().map(Workspace::notes_dir)
    }

    /// Directory that stores screenshots for the open workspace.
    pub fn screenshots_dir(&self) -> Option<&Path> {
        self.workspace.as_ref().map(Workspace::screenshots_dir)
    }

    /// Root directory of the file browser for the open workspace.
    pub fn files_dir(&self) -> Option<&Path> {
        self.workspace.as_ref().map(Workspace::files_dir)
    }

    /// Relative path of the last folder opened in the file browser.
    pub fn last_folder(&self) -> Option<&str> {
        self.workspace.as_ref().and_then(Workspace::last_folder)
    }

    /// Sets and persists the last opened folder.
    ///
    /// The path is relative to the workspace `Files/` folder. `""` and `"."`
    /// both mean the root folder. Absolute paths and `..` traversal are
    /// rejected.
    pub fn set_last_folder(&mut self, relative: &str) -> Result<()> {
        let normalized = workspace::normalize_relative_string(relative)?;
        let folder = if normalized.is_empty() {
            None
        } else {
            Some(normalized)
        };
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .set_last_folder(folder)
    }

    /// Lists notes sorted by modification time, newest first.
    pub fn list_notes(&self) -> Result<Vec<NoteEntry>> {
        self.require_workspace()?.list_notes()
    }

    /// Creates a new empty note with a unique `Untitled.md` style name.
    pub fn create_note(&mut self) -> Result<NoteEntry> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .create_note()
    }

    /// Reads a note as UTF-8 text.
    pub fn read_note(&self, note: &NoteEntry) -> Result<String> {
        self.require_workspace()?.read_note(note)
    }

    /// Writes a note; the file stays a normal `.md` file editable by external
    /// editors.
    pub fn write_note(&self, note: &NoteEntry, contents: &str) -> Result<()> {
        self.require_workspace()?.write_note(note, contents)
    }

    /// Deletes a note. Deleting an already missing note is not an error.
    pub fn delete_note(&self, note: &NoteEntry) -> Result<()> {
        self.require_workspace()?.delete_note(note)
    }

    /// Renames a note and returns the updated entry.
    ///
    /// The `.md` extension is added automatically when `new_name` omits it.
    pub fn rename_note(&mut self, note: &NoteEntry, new_name: &str) -> Result<NoteEntry> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .rename_note(note, new_name)
    }

    /// Lists a folder below `Files/`, directories first and then by name.
    pub fn list_folder(&self, relative: &str) -> Result<Vec<FileEntry>> {
        self.require_workspace()?.list_folder(relative)
    }

    /// Creates a folder below `Files/` and returns the new entry.
    pub fn create_folder(&mut self, relative: &str, name: &str) -> Result<FileEntry> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .create_folder(relative, name)
    }

    /// All sticky notes of the open workspace.
    pub fn stickies(&self) -> &[StickyNote] {
        self.workspace.as_ref().map_or(&[], Workspace::stickies)
    }

    /// Creates and persists a blank sticky note.
    pub fn create_sticky(&mut self) -> Result<StickyNote> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .create_sticky()
    }

    /// Inserts or updates a sticky note and persists the workspace manifest.
    pub fn save_sticky(&mut self, sticky: StickyNote) -> Result<()> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .save_sticky(sticky)
    }

    /// Deletes a sticky note; deleting a missing id is not an error.
    ///
    /// Any matching pinned on-screen sticky is removed as well.
    pub fn delete_sticky(&mut self, sticky_id: &str) -> Result<()> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .delete_sticky(sticky_id)
    }

    /// Pinned on-screen stickies of the open workspace, in manifest order.
    pub fn pinned_stickies(&self) -> &[PinnedSticky] {
        self.workspace
            .as_ref()
            .map_or(&[], Workspace::pinned_stickies)
    }

    /// Returns `true` when the given sticky currently has an on-screen window.
    pub fn is_sticky_pinned(&self, sticky_id: &str) -> bool {
        self.workspace
            .as_ref()
            .is_some_and(|workspace| workspace.is_sticky_pinned(sticky_id))
    }

    /// Pins a sticky to the screen, or updates its geometry when already pinned.
    ///
    /// Pinning an unknown sticky id returns an error. Non-finite coordinates
    /// become `0.0`; the size is clamped to the `PINNED_STICKY_*` bounds.
    pub fn pin_sticky(
        &mut self,
        sticky_id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<()> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .pin_sticky(sticky_id, x, y, width, height)
    }

    /// Unpins a sticky from the screen. Unpinning an unpinned sticky is a no-op.
    pub fn unpin_sticky(&mut self, sticky_id: &str) -> Result<()> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .unpin_sticky(sticky_id)
    }

    /// Moves a pinned sticky to a new screen position and persists it.
    ///
    /// Returns an error when the sticky is not currently pinned. Non-finite
    /// coordinates are replaced with `0.0`.
    pub fn move_pinned_sticky(&mut self, sticky_id: &str, x: f32, y: f32) -> Result<()> {
        self.workspace
            .as_mut()
            .context("no workspace is open")?
            .move_pinned_sticky(sticky_id, x, y)
    }

    /// Saves an RGBA screenshot as a PNG with a unique, time-sortable name.
    pub fn save_screenshot(&self, image: &RgbaImage) -> Result<PathBuf> {
        self.require_workspace()?.save_screenshot(image)
    }

    /// Lists screenshots of the open workspace, newest first.
    pub fn list_screenshots(&self) -> Result<Vec<PathBuf>> {
        self.require_workspace()?.list_screenshots()
    }

    /// Borrows the open workspace or reports a helpful error.
    fn require_workspace(&self) -> Result<&Workspace> {
        self.workspace.as_ref().context("no workspace is open")
    }
}
