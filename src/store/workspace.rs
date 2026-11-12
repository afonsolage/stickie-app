//! Workspace data: manifest, notes, file browser, sticky notes, and screenshots.
//!
//! A workspace lives in its own directory below the app data directory and
//! contains a `workspace.toml` manifest plus the `Notes/`, `Screenshots/`, and
//! `Files/` subdirectories. The manifest is written atomically (temporary file
//! followed by a rename) so a crash can never leave a half-written manifest
//! behind.
//!
//! Everything below is intentionally free of Bevy and platform dependencies so
//! it can be unit tested without a window.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};

/// Name of the file that stores the workspace manifest inside its root folder.
const MANIFEST_FILE: &str = "workspace.toml";
/// Subdirectory that holds markdown notes.
const NOTES_DIR: &str = "Notes";
/// Subdirectory that holds saved screenshots.
const SCREENSHOTS_DIR: &str = "Screenshots";
/// Subdirectory that is exposed as the workspace file browser root.
const FILES_DIR: &str = "Files";
/// Version of the manifest format written by this build.
const MANIFEST_VERSION: u32 = 1;
/// Default width of a freshly created sticky note.
const DEFAULT_STICKY_WIDTH: f32 = 240.0;
/// Default height of a freshly created sticky note.
const DEFAULT_STICKY_HEIGHT: f32 = 180.0;
/// Default width of a pinned on-screen sticky in physical pixels.
const DEFAULT_PINNED_STICKY_WIDTH: f32 = 240.0;
/// Default height of a pinned on-screen sticky in physical pixels.
const DEFAULT_PINNED_STICKY_HEIGHT: f32 = 180.0;
/// Minimum persisted width of a pinned on-screen sticky in physical pixels.
pub const PINNED_STICKY_MIN_WIDTH: f32 = 180.0;
/// Maximum persisted width of a pinned on-screen sticky in physical pixels.
pub const PINNED_STICKY_MAX_WIDTH: f32 = 720.0;
/// Minimum persisted height of a pinned on-screen sticky in physical pixels.
pub const PINNED_STICKY_MIN_HEIGHT: f32 = 100.0;
/// Maximum persisted height of a pinned on-screen sticky in physical pixels.
pub const PINNED_STICKY_MAX_HEIGHT: f32 = 600.0;
/// Longest workspace folder name that is generated from a raw id.
const MAX_WORKSPACE_ID_LEN: usize = 64;
/// Safety valve for unique-name loops; hitting this is a hard error.
const MAX_UNIQUE_ATTEMPTS: u32 = 100_000;

/// Counter used to make temporary file names unique within the process.
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the current unix time in whole seconds, or `0` for clocks before the epoch.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Returns the current unix time in milliseconds, or `0` for clocks before the epoch.
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Converts a [`SystemTime`] into unix seconds, saturating at `0` before the epoch.
fn system_time_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Reads the modification timestamp of `path`, returning `0` when unavailable.
fn path_modified_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_or(0, system_time_secs)
}

/// Writes `bytes` to `path` atomically: a synced temporary file is renamed over
/// the destination. The parent directory is created when missing.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("file {} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let file_name = path.file_name().map_or_else(
        || "data".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(".{file_name}.{}.{unique}.tmp", std::process::id()));

    let write_result = (|| -> Result<()> {
        let mut file = fs::File::create(&temp_path)
            .with_context(|| format!("failed to create temporary file {}", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush temporary file {}", temp_path.display()))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    sync_parent_directory(parent);
    Ok(())
}

/// Best-effort directory sync so a rename survives a power loss on unix.
fn sync_parent_directory(parent: &Path) {
    #[cfg(unix)]
    {
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
    }
}

/// Renames `path` to `<name>.bak`, replacing an older backup when present.
pub(crate) fn backup_file(path: &Path) -> Result<PathBuf> {
    let mut name = path
        .file_name()
        .map_or_else(|| OsString::from("data"), OsStr::to_os_string);
    name.push(".bak");
    let backup = path.with_file_name(name);
    match fs::remove_file(&backup) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to replace backup {}", backup.display()));
        }
    }
    fs::rename(path, &backup).with_context(|| {
        format!(
            "failed to back up {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(backup)
}

/// Splits a relative path into validated segments.
///
/// Both `/` and `\` are treated as separators so a workspace manifest written
/// on Windows stays safe when read on another platform (and vice versa).
fn path_segments(relative: &str) -> Result<Vec<&str>> {
    if relative.starts_with('/') || relative.starts_with('\\') || Path::new(relative).is_absolute()
    {
        bail!("absolute paths are not allowed: {relative:?}");
    }
    let mut segments = Vec::new();
    for raw in relative.split(['/', '\\']) {
        if raw.is_empty() || raw == "." {
            continue;
        }
        if raw == ".." {
            bail!("path traversal is not allowed: {relative:?}");
        }
        if raw.contains(':') || raw.contains('\0') {
            bail!("invalid path component {raw:?} in {relative:?}");
        }
        segments.push(raw);
    }
    Ok(segments)
}

/// Normalises a relative path, rejecting absolute paths and `..` traversal.
pub(crate) fn normalize_relative(relative: &str) -> Result<PathBuf> {
    let mut path = PathBuf::new();
    for segment in path_segments(relative)? {
        path.push(segment);
    }
    Ok(path)
}

/// Normalises a relative path into a forward-slash separated string suitable
/// for storing in the manifest.
pub(crate) fn normalize_relative_string(relative: &str) -> Result<String> {
    Ok(path_segments(relative)?.join("/"))
}

/// Validates a single file or folder name (no separators, no traversal).
fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("name cannot be empty");
    }
    if trimmed == "." || trimmed == ".." {
        bail!("invalid name {name:?}");
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, '/' | '\\' | ':' | '\0'))
    {
        bail!("name {name:?} cannot contain path separators");
    }
    Ok(trimmed.to_string())
}

/// Ensures `path` is inside `base` after resolving symlinks.
///
/// The deepest existing ancestor of `path` is canonicalised, which catches both
/// lexical escapes and symlinks that point outside of the workspace root.
fn ensure_contained(base: &Path, path: &Path) -> Result<()> {
    let canonical_base = base
        .canonicalize()
        .with_context(|| format!("workspace directory {} is unavailable", base.display()))?;
    let mut probe = path;
    loop {
        match probe.canonicalize() {
            Ok(canonical) => {
                if !canonical.starts_with(&canonical_base) {
                    bail!("path {} escapes the workspace directory", path.display());
                }
                return Ok(());
            }
            Err(_) => match probe.parent() {
                Some(parent) if parent.starts_with(base) => probe = parent,
                _ => bail!("invalid workspace path {}", path.display()),
            },
        }
    }
}

/// Resolves a path relative to `base`, rejecting traversal and symlink escapes.
pub(crate) fn resolve_under(base: &Path, relative: &str) -> Result<PathBuf> {
    let relative_path = normalize_relative(relative)?;
    let path = if relative_path.as_os_str().is_empty() {
        base.to_path_buf()
    } else {
        base.join(relative_path)
    };
    ensure_contained(base, &path)?;
    Ok(path)
}

/// Describes the workspace that should be opened for the foreground game.
///
/// The platform layer maps its `GameContext` onto this descriptor; the store
/// sanitises [`WorkspaceDescriptor::workspace_id`] into a folder name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDescriptor<'a> {
    /// Stable identifier for the game, for example `exe:eldenring`.
    pub workspace_id: &'a str,
    /// Human readable name shown in the UI.
    pub display_name: &'a str,
    /// Optional executable path or file name that produced the workspace.
    pub executable: Option<&'a str>,
    /// Optional foreground window title that produced the workspace.
    pub window_title: Option<&'a str>,
}

impl<'a> WorkspaceDescriptor<'a> {
    /// Creates a descriptor from a workspace id and display name.
    pub fn new(workspace_id: &'a str, display_name: &'a str) -> Self {
        Self {
            workspace_id,
            display_name,
            executable: None,
            window_title: None,
        }
    }

    /// Sets the executable metadata associated with the workspace.
    #[must_use]
    pub fn with_executable(mut self, executable: Option<&'a str>) -> Self {
        self.executable = executable;
        self
    }

    /// Sets the window title metadata associated with the workspace.
    #[must_use]
    pub fn with_window_title(mut self, window_title: Option<&'a str>) -> Self {
        self.window_title = window_title;
        self
    }
}

/// Sanitises a raw workspace id into a filesystem-safe folder name.
///
/// The result only contains lowercase ASCII letters, digits, and `-`. Runs of
/// invalid characters collapse into a single dash, leading and trailing dashes
/// are dropped, the result is capped at 64 characters, and an empty result
/// becomes `"desktop"`.
pub fn sanitize_workspace_id(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    let mut pending_dash = false;
    for character in raw.chars() {
        let lower = character.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            if pending_dash && !sanitized.is_empty() {
                sanitized.push('-');
            }
            pending_dash = false;
            sanitized.push(lower);
        } else {
            pending_dash = true;
        }
    }
    if sanitized.len() > MAX_WORKSPACE_ID_LEN {
        sanitized.truncate(MAX_WORKSPACE_ID_LEN);
        while sanitized.ends_with('-') {
            sanitized.pop();
        }
    }
    if sanitized.is_empty() {
        "desktop".to_string()
    } else {
        sanitized
    }
}

/// Palette available for sticky notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StickyColor {
    /// Warm yellow.
    #[default]
    Amber,
    /// Soft green.
    Mint,
    /// Light blue.
    Sky,
    /// Dusty pink.
    Rose,
    /// Light purple.
    Lilac,
    /// Neutral off-white.
    Paper,
}

impl StickyColor {
    /// Every colour in display order, useful for a palette picker.
    pub const ALL: [Self; 6] = [
        Self::Amber,
        Self::Mint,
        Self::Sky,
        Self::Rose,
        Self::Lilac,
        Self::Paper,
    ];
}

/// A sticky note pinned to a workspace board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickyNote {
    /// Stable identifier; generated by the store and never reused.
    pub id: String,
    /// Note text. Newlines are kept verbatim.
    pub text: String,
    /// Card colour.
    pub color: StickyColor,
    /// X position on the overlay board.
    pub x: f32,
    /// Y position on the overlay board.
    pub y: f32,
    /// Card width in logical pixels.
    pub width: f32,
    /// Card height in logical pixels.
    pub height: f32,
    /// Creation time in unix seconds.
    pub created_at: u64,
    /// Last modification time in unix seconds.
    pub updated_at: u64,
}

impl Default for StickyNote {
    fn default() -> Self {
        Self {
            id: String::new(),
            text: String::new(),
            color: StickyColor::default(),
            x: 0.0,
            y: 0.0,
            width: DEFAULT_STICKY_WIDTH,
            height: DEFAULT_STICKY_HEIGHT,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// A sticky note that is also rendered in a small always-on-top OS window.
///
/// Geometry is stored in physical pixels so it survives DPI changes on the
/// monitor the window was placed on. The store clamps sizes to sane bounds and
/// replaces non-finite values, so readers never observe NaN rectangles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinnedSticky {
    /// Id of the [`StickyNote`] this pin renders.
    pub sticky_id: String,
    /// Screen x position in physical pixels.
    pub x: f32,
    /// Screen y position in physical pixels.
    pub y: f32,
    /// Window width in physical pixels, clamped between
    /// [`PINNED_STICKY_MIN_WIDTH`] and [`PINNED_STICKY_MAX_WIDTH`].
    pub width: f32,
    /// Window height in physical pixels, clamped between
    /// [`PINNED_STICKY_MIN_HEIGHT`] and [`PINNED_STICKY_MAX_HEIGHT`].
    pub height: f32,
}

impl Default for PinnedSticky {
    fn default() -> Self {
        Self {
            sticky_id: String::new(),
            x: 0.0,
            y: 0.0,
            width: DEFAULT_PINNED_STICKY_WIDTH,
            height: DEFAULT_PINNED_STICKY_HEIGHT,
        }
    }
}

/// A markdown note stored as a plain `.md` file in the workspace `Notes/` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    /// Absolute path of the note file.
    pub path: PathBuf,
    /// Path relative to the workspace `Notes/` folder (usually just the file name).
    pub relative: PathBuf,
    /// Display title derived from the file stem.
    pub title: String,
    /// Last modification time in unix seconds.
    pub modified_secs: u64,
}

/// An entry shown by the workspace file browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Absolute path of the entry.
    pub path: PathBuf,
    /// Path relative to the workspace `Files/` folder.
    pub relative: PathBuf,
    /// File or folder name.
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Last modification time in unix seconds.
    pub modified_secs: u64,
}

/// On-disk representation of a workspace manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Manifest {
    version: u32,
    id: String,
    display_name: String,
    executable: Option<String>,
    window_title: Option<String>,
    created_at: u64,
    updated_at: u64,
    last_folder: Option<String>,
    stickies: Vec<StickyNote>,
    #[serde(default, deserialize_with = "deserialize_pinned_stickies")]
    pinned_stickies: Vec<PinnedSticky>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            id: String::new(),
            display_name: String::new(),
            executable: None,
            window_title: None,
            created_at: 0,
            updated_at: 0,
            last_folder: None,
            stickies: Vec::new(),
            pinned_stickies: Vec::new(),
        }
    }
}

/// An open workspace rooted in a directory on disk.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    notes_dir: PathBuf,
    screenshots_dir: PathBuf,
    files_dir: PathBuf,
    id: String,
    display_name: String,
    executable: Option<String>,
    window_title: Option<String>,
    created_at: u64,
    updated_at: u64,
    last_folder: Option<String>,
    stickies: Vec<StickyNote>,
    pinned_stickies: Vec<PinnedSticky>,
}

impl Workspace {
    /// Opens (or creates) the workspace rooted at `root` and refreshes the
    /// metadata that comes from the foreground game.
    pub(crate) fn open(root: PathBuf, descriptor: WorkspaceDescriptor<'_>) -> Result<Self> {
        let notes_dir = root.join(NOTES_DIR);
        let screenshots_dir = root.join(SCREENSHOTS_DIR);
        let files_dir = root.join(FILES_DIR);
        for directory in [&root, &notes_dir, &screenshots_dir, &files_dir] {
            fs::create_dir_all(directory).with_context(|| {
                format!(
                    "failed to create workspace directory {}",
                    directory.display()
                )
            })?;
        }

        let existing = read_manifest(&root.join(MANIFEST_FILE))?;
        let id = sanitize_workspace_id(descriptor.workspace_id);
        let display_name = non_empty_trimmed(descriptor.display_name)
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|manifest| non_empty_trimmed(&manifest.display_name))
            })
            .unwrap_or_else(|| fallback_display_name(&id));
        let executable = non_empty_trimmed_option(descriptor.executable).or_else(|| {
            existing
                .as_ref()
                .and_then(|manifest| non_empty_trimmed_option(manifest.executable.as_deref()))
        });
        let window_title = non_empty_trimmed_option(descriptor.window_title).or_else(|| {
            existing
                .as_ref()
                .and_then(|manifest| non_empty_trimmed_option(manifest.window_title.as_deref()))
        });

        let now = now_unix_secs();
        let (created_at, last_folder, stickies, pinned_stickies) = match existing {
            Some(manifest) => {
                if manifest.version > MANIFEST_VERSION {
                    log::warn!(
                        "workspace manifest version {} is newer than supported version {}; \
                         loading it on a best-effort basis",
                        manifest.version,
                        MANIFEST_VERSION
                    );
                }
                let last_folder = manifest
                    .last_folder
                    .as_deref()
                    .and_then(|value| normalize_relative_string(value).ok())
                    .filter(|value| !value.is_empty());
                let stickies = manifest.stickies;
                // Drop pins that point at stickies which no longer exist and
                // repair malformed geometry before the manifest is rewritten.
                let pinned_stickies = sanitize_pinned_stickies(manifest.pinned_stickies, &stickies);
                (
                    if manifest.created_at == 0 {
                        now
                    } else {
                        manifest.created_at
                    },
                    last_folder,
                    stickies,
                    pinned_stickies,
                )
            }
            None => (now, None, Vec::new(), Vec::new()),
        };

        let workspace = Self {
            root,
            notes_dir,
            screenshots_dir,
            files_dir,
            id,
            display_name,
            executable,
            window_title,
            created_at,
            updated_at: now,
            last_folder,
            stickies,
            pinned_stickies,
        };
        workspace.persist()?;
        Ok(workspace)
    }

    /// Writes the manifest atomically.
    pub(crate) fn persist(&self) -> Result<()> {
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            executable: self.executable.clone(),
            window_title: self.window_title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_folder: self.last_folder.clone(),
            stickies: self.stickies.clone(),
            pinned_stickies: self.pinned_stickies.clone(),
        };
        let contents =
            toml::to_string_pretty(&manifest).context("failed to serialize workspace manifest")?;
        write_atomic(&self.root.join(MANIFEST_FILE), contents.as_bytes())
    }

    /// Stable workspace id (also the folder name under `workspaces/`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Human readable workspace name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Executable associated with the workspace, when known.
    pub fn executable(&self) -> Option<&str> {
        self.executable.as_deref()
    }

    /// Foreground window title associated with the workspace, when known.
    pub fn window_title(&self) -> Option<&str> {
        self.window_title.as_deref()
    }

    /// Unix seconds when the workspace was first created.
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Unix seconds of the last manifest update.
    pub fn updated_at(&self) -> u64 {
        self.updated_at
    }

    /// Workspace root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory that stores markdown notes.
    pub fn notes_dir(&self) -> &Path {
        &self.notes_dir
    }

    /// Directory that stores screenshots.
    pub fn screenshots_dir(&self) -> &Path {
        &self.screenshots_dir
    }

    /// Root directory of the workspace file browser.
    pub fn files_dir(&self) -> &Path {
        &self.files_dir
    }

    /// Relative path of the last folder opened in the file browser.
    pub fn last_folder(&self) -> Option<&str> {
        self.last_folder.as_deref()
    }

    /// All sticky notes attached to this workspace.
    pub fn stickies(&self) -> &[StickyNote] {
        &self.stickies
    }

    /// Pinned on-screen stickies attached to this workspace, in manifest order.
    pub fn pinned_stickies(&self) -> &[PinnedSticky] {
        &self.pinned_stickies
    }

    /// Returns `true` when `sticky_id` currently has an on-screen window.
    pub fn is_sticky_pinned(&self, sticky_id: &str) -> bool {
        self.pinned_stickies
            .iter()
            .any(|pinned| pinned.sticky_id == sticky_id)
    }

    /// Pins a sticky to the screen, or updates its geometry when already pinned.
    ///
    /// `sticky_id` must refer to an existing sticky note; pinning an unknown id
    /// returns an error. Non-finite coordinates become `0.0`, widths are
    /// clamped between [`PINNED_STICKY_MIN_WIDTH`] and
    /// [`PINNED_STICKY_MAX_WIDTH`], and heights between
    /// [`PINNED_STICKY_MIN_HEIGHT`] and [`PINNED_STICKY_MAX_HEIGHT`].
    pub fn pin_sticky(
        &mut self,
        sticky_id: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<()> {
        if !self.stickies.iter().any(|sticky| sticky.id == sticky_id) {
            bail!("cannot pin unknown sticky note {sticky_id:?}");
        }
        let mut pinned = PinnedSticky {
            sticky_id: sticky_id.to_string(),
            x,
            y,
            width,
            height,
        };
        sanitize_pinned_geometry(&mut pinned);
        match self
            .pinned_stickies
            .iter_mut()
            .find(|existing| existing.sticky_id == pinned.sticky_id)
        {
            Some(existing) => *existing = pinned,
            None => self.pinned_stickies.push(pinned),
        }
        self.updated_at = now_unix_secs();
        self.persist()
    }

    /// Unpins a sticky from the screen and persists the manifest.
    ///
    /// Unpinning a sticky that is not pinned is a no-op.
    pub fn unpin_sticky(&mut self, sticky_id: &str) -> Result<()> {
        let previous_len = self.pinned_stickies.len();
        self.pinned_stickies
            .retain(|pinned| pinned.sticky_id != sticky_id);
        if self.pinned_stickies.len() != previous_len {
            self.updated_at = now_unix_secs();
            self.persist()?;
        }
        Ok(())
    }

    /// Moves a pinned sticky to a new screen position and persists the manifest.
    ///
    /// Returns an error when `sticky_id` is not currently pinned. Non-finite
    /// coordinates are replaced with `0.0`.
    pub fn move_pinned_sticky(&mut self, sticky_id: &str, x: f32, y: f32) -> Result<()> {
        let pinned = self
            .pinned_stickies
            .iter_mut()
            .find(|pinned| pinned.sticky_id == sticky_id)
            .with_context(|| format!("sticky note {sticky_id:?} is not pinned"))?;
        pinned.x = if x.is_finite() { x } else { 0.0 };
        pinned.y = if y.is_finite() { y } else { 0.0 };
        self.updated_at = now_unix_secs();
        self.persist()
    }

    /// Lists notes sorted by modification time, newest first.
    pub(crate) fn list_notes(&self) -> Result<Vec<NoteEntry>> {
        let directory = fs::read_dir(&self.notes_dir).with_context(|| {
            format!(
                "failed to read notes directory {}",
                self.notes_dir.display()
            )
        })?;
        let mut notes = Vec::new();
        for entry in directory {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read notes directory {}",
                    self.notes_dir.display()
                )
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to inspect note {}", path.display()))?;
            if !file_type.is_file() || !is_markdown(&path) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            notes.push((modified, self.note_entry(&path)?));
        }
        notes.sort_by(|(modified_a, note_a), (modified_b, note_b)| {
            modified_b
                .cmp(modified_a)
                .then_with(|| {
                    note_a
                        .title
                        .to_lowercase()
                        .cmp(&note_b.title.to_lowercase())
                })
                .then_with(|| note_a.path.cmp(&note_b.path))
        });
        Ok(notes.into_iter().map(|(_, note)| note).collect())
    }

    /// Creates a new empty note with a unique name.
    pub(crate) fn create_note(&mut self) -> Result<NoteEntry> {
        for index in 1..=MAX_UNIQUE_ATTEMPTS {
            let name = if index == 1 {
                "Untitled.md".to_string()
            } else {
                format!("Untitled {index}.md")
            };
            let path = self.notes_dir.join(&name);
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    drop(file);
                    return self.note_entry(&path);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to create note {}", path.display()));
                }
            }
        }
        bail!(
            "could not find a free note name in {}",
            self.notes_dir.display()
        )
    }

    /// Reads a note as UTF-8 text.
    pub(crate) fn read_note(&self, note: &NoteEntry) -> Result<String> {
        let path = self.note_path(note)?;
        fs::read_to_string(&path).with_context(|| format!("failed to read note {}", path.display()))
    }

    /// Writes a note durably enough for external editors to observe the change
    /// immediately; the file remains a normal `.md` file on disk.
    pub(crate) fn write_note(&self, note: &NoteEntry, contents: &str) -> Result<()> {
        let path = self.note_path(note)?;
        let mut file = fs::File::create(&path)
            .with_context(|| format!("failed to open note {} for writing", path.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write note {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush note {}", path.display()))?;
        Ok(())
    }

    /// Deletes a note. Deleting an already missing note is not an error.
    pub(crate) fn delete_note(&self, note: &NoteEntry) -> Result<()> {
        let path = self.note_path(note)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to delete note {}", path.display()))
            }
        }
    }

    /// Renames a note, adding the `.md` extension when the caller omits it.
    pub(crate) fn rename_note(&self, note: &NoteEntry, new_name: &str) -> Result<NoteEntry> {
        let current_path = self.note_path(note)?;
        let cleaned = validate_name(new_name)?;
        let file_name = if cleaned.to_ascii_lowercase().ends_with(".md") {
            cleaned
        } else {
            format!("{cleaned}.md")
        };
        let target = self.notes_dir.join(&file_name);
        ensure_contained(&self.notes_dir, &target)?;
        if target == current_path {
            return self.note_entry(&current_path);
        }
        if target.exists() {
            bail!("a note named {file_name:?} already exists");
        }
        fs::rename(&current_path, &target).with_context(|| {
            format!(
                "failed to rename note {} to {}",
                current_path.display(),
                target.display()
            )
        })?;
        self.note_entry(&target)
    }

    /// Validates that an entry points inside this workspace's notes folder.
    fn note_path(&self, note: &NoteEntry) -> Result<PathBuf> {
        ensure_contained(&self.notes_dir, &note.path)?;
        Ok(note.path.clone())
    }

    /// Builds a [`NoteEntry`] for an existing note file.
    fn note_entry(&self, path: &Path) -> Result<NoteEntry> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to inspect note {}", path.display()))?;
        let relative = path.strip_prefix(&self.notes_dir).map_or_else(
            |_| {
                PathBuf::from(
                    path.file_name()
                        .map(OsStr::to_os_string)
                        .unwrap_or_default(),
                )
            },
            Path::to_path_buf,
        );
        Ok(NoteEntry {
            path: path.to_path_buf(),
            relative,
            title: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            modified_secs: metadata
                .modified()
                .map_or_else(|_| path_modified_secs(path), system_time_secs),
        })
    }

    /// Lists a folder below `Files/`, directories first and then by name.
    pub(crate) fn list_folder(&self, relative: &str) -> Result<Vec<FileEntry>> {
        let directory = resolve_under(&self.files_dir, relative)?;
        let read_dir = fs::read_dir(&directory)
            .with_context(|| format!("failed to read folder {}", directory.display()))?;
        let mut entries = Vec::new();
        for entry in read_dir {
            let entry =
                entry.with_context(|| format!("failed to read folder {}", directory.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to inspect {}", path.display()))?;
            // Follow symlinks for display purposes; resolving the path when the
            // folder is entered still rejects symlinks that escape the workspace.
            let is_dir = file_type.is_dir() || path.is_dir();
            let metadata = entry.metadata();
            let modified = metadata
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            entries.push(FileEntry {
                path: path.clone(),
                relative: path
                    .strip_prefix(&self.files_dir)
                    .map_or_else(|_| path.clone(), Path::to_path_buf),
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir,
                modified_secs: system_time_secs(modified),
            });
        }
        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    /// Creates a folder below `Files/` and returns the new entry.
    pub(crate) fn create_folder(&self, relative: &str, name: &str) -> Result<FileEntry> {
        let parent = resolve_under(&self.files_dir, relative)?;
        let folder_name = validate_name(name)?;
        let path = parent.join(&folder_name);
        ensure_contained(&self.files_dir, &path)?;
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create folder {}", path.display()))?;
        let relative_path = path
            .strip_prefix(&self.files_dir)
            .map_or_else(|_| path.clone(), Path::to_path_buf);
        Ok(FileEntry {
            modified_secs: path_modified_secs(&path),
            path,
            relative: relative_path,
            name: folder_name,
            is_dir: true,
        })
    }

    /// Updates the last opened folder and persists the manifest.
    pub(crate) fn set_last_folder(&mut self, folder: Option<String>) -> Result<()> {
        if self.last_folder == folder {
            return Ok(());
        }
        self.last_folder = folder;
        self.updated_at = now_unix_secs();
        self.persist()
    }

    /// Creates and persists a blank sticky note.
    pub(crate) fn create_sticky(&mut self) -> Result<StickyNote> {
        let now = now_unix_secs();
        let sticky = StickyNote {
            id: self.next_sticky_id(),
            text: String::new(),
            color: StickyColor::default(),
            x: 0.0,
            y: 0.0,
            width: DEFAULT_STICKY_WIDTH,
            height: DEFAULT_STICKY_HEIGHT,
            created_at: now,
            updated_at: now,
        };
        self.stickies.push(sticky.clone());
        self.updated_at = now;
        self.persist()?;
        Ok(sticky)
    }

    /// Inserts or updates a sticky note and persists the manifest.
    pub(crate) fn save_sticky(&mut self, mut sticky: StickyNote) -> Result<()> {
        let now = now_unix_secs();
        if sticky.id.trim().is_empty() {
            sticky.id = self.next_sticky_id();
        }
        sanitize_geometry(&mut sticky);
        if sticky.created_at == 0 {
            sticky.created_at = now;
        }
        sticky.updated_at = now;
        match self
            .stickies
            .iter_mut()
            .find(|existing| existing.id == sticky.id)
        {
            Some(existing) => {
                if existing.created_at != 0 {
                    sticky.created_at = existing.created_at;
                }
                *existing = sticky;
            }
            None => self.stickies.push(sticky),
        }
        self.updated_at = now;
        self.persist()
    }

    /// Deletes a sticky note; deleting a missing id is not an error.
    ///
    /// Any matching pinned on-screen sticky is removed as well.
    pub(crate) fn delete_sticky(&mut self, sticky_id: &str) -> Result<()> {
        let previous_stickies = self.stickies.len();
        let previous_pins = self.pinned_stickies.len();
        self.stickies.retain(|sticky| sticky.id != sticky_id);
        self.pinned_stickies
            .retain(|pinned| pinned.sticky_id != sticky_id);
        if self.stickies.len() != previous_stickies || self.pinned_stickies.len() != previous_pins {
            self.updated_at = now_unix_secs();
            self.persist()?;
        }
        Ok(())
    }

    /// Generates a unique, stable sticky note id without pulling in a uuid crate.
    fn next_sticky_id(&self) -> String {
        let stamp = now_unix_millis();
        for index in 1..=MAX_UNIQUE_ATTEMPTS {
            let candidate = if index == 1 {
                format!("sticky-{stamp}")
            } else {
                format!("sticky-{stamp}-{index}")
            };
            if !self.stickies.iter().any(|sticky| sticky.id == candidate) {
                return candidate;
            }
        }
        let fallback = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("sticky-{stamp}-{fallback}")
    }

    /// Saves an RGBA screenshot as a PNG with a unique, time-sortable name.
    pub(crate) fn save_screenshot(&self, image: &RgbaImage) -> Result<PathBuf> {
        let seconds = now_unix_secs();
        for index in 1..=MAX_UNIQUE_ATTEMPTS {
            let name = if index == 1 {
                format!("{seconds}-screenshot.png")
            } else {
                format!("{seconds}-{index}-screenshot.png")
            };
            let path = self.screenshots_dir.join(name);
            if path.exists() {
                continue;
            }
            image
                .save_with_format(&path, ImageFormat::Png)
                .with_context(|| format!("failed to save screenshot {}", path.display()))?;
            return Ok(path);
        }
        bail!(
            "could not find a free screenshot file name in {}",
            self.screenshots_dir.display()
        )
    }

    /// Lists screenshots, newest first.
    pub(crate) fn list_screenshots(&self) -> Result<Vec<PathBuf>> {
        let read_dir = fs::read_dir(&self.screenshots_dir).with_context(|| {
            format!(
                "failed to read screenshots directory {}",
                self.screenshots_dir.display()
            )
        })?;
        let mut screenshots = Vec::new();
        for entry in read_dir {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read screenshots directory {}",
                    self.screenshots_dir.display()
                )
            })?;
            let path = entry.path();
            let is_png = path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
            if !path.is_file() || !is_png {
                continue;
            }
            let stem = path.file_stem().and_then(OsStr::to_str).unwrap_or_default();
            screenshots.push((ScreenshotOrder::parse(stem, &path), path));
        }
        screenshots.sort_by(|(order_a, path_a), (order_b, path_b)| {
            order_b.cmp(order_a).then_with(|| path_b.cmp(path_a))
        });
        Ok(screenshots.into_iter().map(|(_, path)| path).collect())
    }
}

/// Sort key for screenshot file names of the form `<unixsecs>[-<n>]-screenshot.png`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ScreenshotOrder {
    seconds: u64,
    index: u64,
}

impl ScreenshotOrder {
    fn parse(stem: &str, path: &Path) -> Self {
        let mut parts = stem.split('-');
        let seconds = parts.next().and_then(|part| part.parse::<u64>().ok());
        let index = parts.next().and_then(|part| part.parse::<u64>().ok());
        match (seconds, index) {
            (Some(seconds), Some(index)) => Self { seconds, index },
            (Some(seconds), None) => Self { seconds, index: 1 },
            (None, _) => Self {
                seconds: path_modified_secs(path),
                index: 0,
            },
        }
    }
}

/// Returns `true` when the path has a `.md` extension (case insensitive).
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

/// Reads the workspace manifest, recovering from malformed files by backing
/// them up and starting a fresh workspace.
fn read_manifest(path: &Path) -> Result<Option<Manifest>> {
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<Manifest>(&contents) {
            Ok(manifest) => Ok(Some(manifest)),
            Err(error) => {
                log::warn!(
                    "workspace manifest {} is malformed ({error}); backing it up and starting a \
                     fresh workspace",
                    path.display()
                );
                if let Err(backup_error) = backup_file(path) {
                    log::warn!("could not back up malformed workspace manifest: {backup_error:#}");
                }
                Ok(None)
            }
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read workspace manifest {}", path.display())),
    }
}

/// Trims a string, returning `None` when it is empty.
fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Maps an optional string through [`non_empty_trimmed`].
fn non_empty_trimmed_option(value: Option<&str>) -> Option<String> {
    value.and_then(non_empty_trimmed)
}

/// Turns `exe-elden-ring` into `Exe Elden Ring` for use as a fallback name.
fn fallback_display_name(id: &str) -> String {
    let mut name = String::with_capacity(id.len());
    let mut capitalize = true;
    for character in id.chars() {
        if character == '-' {
            name.push(' ');
            capitalize = true;
        } else if capitalize {
            name.extend(character.to_uppercase());
            capitalize = false;
        } else {
            name.push(character);
        }
    }
    if name.is_empty() {
        "Desktop".to_string()
    } else {
        name
    }
}

/// Replaces non-finite or nonsensical sticky geometry with defaults so the
/// manifest stays usable and the UI never receives NaN rectangles.
fn sanitize_geometry(sticky: &mut StickyNote) {
    if !sticky.x.is_finite() {
        sticky.x = 0.0;
    }
    if !sticky.y.is_finite() {
        sticky.y = 0.0;
    }
    if !sticky.width.is_finite() || sticky.width <= 0.0 {
        sticky.width = DEFAULT_STICKY_WIDTH;
    }
    if !sticky.height.is_finite() || sticky.height <= 0.0 {
        sticky.height = DEFAULT_STICKY_HEIGHT;
    }
}

/// Sanitises the persisted geometry of a pinned on-screen sticky.
fn sanitize_pinned_geometry(pinned: &mut PinnedSticky) {
    if !pinned.x.is_finite() {
        pinned.x = 0.0;
    }
    if !pinned.y.is_finite() {
        pinned.y = 0.0;
    }
    pinned.width = if pinned.width.is_finite() {
        pinned
            .width
            .clamp(PINNED_STICKY_MIN_WIDTH, PINNED_STICKY_MAX_WIDTH)
    } else {
        DEFAULT_PINNED_STICKY_WIDTH
    };
    pinned.height = if pinned.height.is_finite() {
        pinned
            .height
            .clamp(PINNED_STICKY_MIN_HEIGHT, PINNED_STICKY_MAX_HEIGHT)
    } else {
        DEFAULT_PINNED_STICKY_HEIGHT
    };
}

/// Drops stale or duplicate pins and repairs geometry after loading a manifest.
///
/// A pin is stale when its sticky note no longer exists. Duplicate ids keep the
/// last manifest entry so the most recent geometry wins.
fn sanitize_pinned_stickies(pins: Vec<PinnedSticky>, stickies: &[StickyNote]) -> Vec<PinnedSticky> {
    let mut sanitized: Vec<PinnedSticky> = Vec::with_capacity(pins.len());
    for mut pinned in pins {
        if pinned.sticky_id.trim().is_empty()
            || !stickies.iter().any(|sticky| sticky.id == pinned.sticky_id)
        {
            continue;
        }
        sanitize_pinned_geometry(&mut pinned);
        match sanitized
            .iter_mut()
            .find(|existing| existing.sticky_id == pinned.sticky_id)
        {
            Some(existing) => *existing = pinned,
            None => sanitized.push(pinned),
        }
    }
    sanitized
}

/// Tolerant deserialiser for the manifest `pinned_stickies` array.
///
/// A single malformed entry is logged and dropped instead of failing the whole
/// manifest (which would trigger the recovery path and hide the user's stickies).
fn deserialize_pinned_stickies<'de, D>(deserializer: D) -> Result<Vec<PinnedSticky>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<toml::Value>::deserialize(deserializer)?;
    let mut pinned = Vec::with_capacity(values.len());
    for value in values {
        match PinnedSticky::deserialize(value) {
            Ok(entry) => pinned.push(entry),
            Err(error) => log::warn!("dropping malformed pinned sticky entry: {error}"),
        }
    }
    Ok(pinned)
}
