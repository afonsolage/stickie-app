//! Platform facade for Stickie.
//!
//! Everything that is specific to the operating system is isolated here:
//!
//! - foreground-game probing ([`active_game`]),
//! - screenshot capture ([`capture_game`]),
//! - restoring focus to the game after the overlay closes
//!   ([`restore_game_focus`]),
//! - revealing files and folders in the OS file manager,
//! - the global toggle hotkey (see the [`hotkey`] module).
//!
//! The real Windows implementation lives in the private `windows` module and is
//! only compiled on Windows. On other platforms the `stub` module keeps the
//! exact same signatures but returns [`None`]/unsupported errors so the UI and
//! store can be developed on Linux.
//!
//! String manipulation and decision helpers that do not touch the OS (workspace
//! id sanitising, display-name prettifying, shell-window filtering, ...) live
//! in this file so they can be unit tested on every host.
//!
//! # Safety
//!
//! The crate denies `unsafe_code` globally. The only modules allowed to relax
//! that are the Windows-only implementations, next to clearly commented SAFETY
//! blocks.

pub mod hotkey;

#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use self::windows::{
    active_game, capture_game, capture_screen, configure_overlay_graphics,
    foreground_is_own_process, open_in_file_manager, primary_monitor_size, restore_game_focus,
    reveal_in_file_manager,
};

#[cfg(not(windows))]
pub use self::stub::{
    active_game, capture_game, capture_screen, configure_overlay_graphics,
    foreground_is_own_process, open_in_file_manager, primary_monitor_size, restore_game_focus,
    reveal_in_file_manager,
};

use std::path::Path;

/// The identity of the application currently in the foreground.
///
/// A workspace is keyed by the game's executable so that it is stable across
/// launches and window-title changes:
///
/// ```text
/// game_id = "exe:" + sanitized exe stem    // e.g. "exe:eldenring"
/// ```
///
/// Everything except [`workspace_id`](Self::workspace_id) and
/// [`display_name`](Self::display_name) is best effort: the foreground process
/// may be protected, so partial data is preferred over failing the probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameContext {
    /// Stable, sanitised workspace id, e.g. `exe:eldenring`.
    pub workspace_id: String,
    /// Human readable name derived from the window title or the executable.
    pub display_name: String,
    /// Full path to the game executable, when it could be queried.
    pub executable: Option<String>,
    /// Foreground window title, when it is not empty.
    pub window_title: Option<String>,
    /// Process id of the foreground window.
    pub process_id: Option<u32>,
    /// Raw `HWND` of the foreground window, for [`restore_game_focus`].
    pub window_handle: Option<isize>,
}

/// Returns `true` when this build can host the overlay and capture the screen.
///
/// The product is Windows-only for now; this is a `const fn` so UI code can use
/// it in `const` contexts and `if cfg!`-style decisions.
pub const fn overlay_supported() -> bool {
    cfg!(windows)
}

/// Short, stable name of the current platform (`"windows"`, `"linux"`, ...).
pub fn platform_name() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        std::env::consts::OS
    }
}

/// Maximum number of characters kept from a window title when it is used as a
/// [`GameContext::display_name`].
pub const MAX_DISPLAY_NAME_CHARS: usize = 80;

/// Minimum width, in pixels, of a foreground window before it is considered
/// game-like. Toolbars, popups and splash helpers are much smaller than this.
pub const MIN_GAME_WINDOW_WIDTH: i32 = 320;

/// Minimum height, in pixels, of a foreground window before it is considered
/// game-like.
pub const MIN_GAME_WINDOW_HEIGHT: i32 = 240;

/// Window classes that belong to the Windows shell/desktop rather than to an
/// application window. Probing the foreground window must never create a
/// workspace for the desktop (`Progman`) or the taskbar.
pub const SHELL_WINDOW_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
];

/// Executable names of Windows shell components. The foreground window of the
/// desktop or Start menu is hosted by these processes, so they are skipped even
/// though they are ordinary windows with titles.
pub const SHELL_PROCESS_NAMES: &[&str] = &[
    "explorer.exe",
    "searchhost.exe",
    "searchapp.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
    "lockapp.exe",
    "logonui.exe",
    "winlogon.exe",
    "dwm.exe",
    "csrss.exe",
    "applicationframehost.exe",
    "sihost.exe",
    "ctfmon.exe",
    "widgetservice.exe",
    "widgets.exe",
];

/// Sanitises a free-form string into a workspace-id component.
///
/// The result only contains lowercase ASCII letters, ASCII digits and `-`.
/// Runs of other characters collapse into a single dash, dashes are never
/// leading or trailing, and non-ASCII characters are treated as separators so
/// the id is always filesystem and URL safe.
///
/// ```
/// # use stickie::platform::sanitize_id_component;
/// assert_eq!(sanitize_id_component("ELDEN RING™"), "elden-ring");
/// assert_eq!(sanitize_id_component("dark_souls_3"), "dark-souls-3");
/// assert_eq!(sanitize_id_component("  ---  "), "");
/// ```
pub fn sanitize_id_component(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    let mut pending_dash = false;
    for byte in raw.bytes() {
        let byte = byte.to_ascii_lowercase();
        if byte.is_ascii_alphanumeric() {
            if pending_dash && !sanitized.is_empty() {
                sanitized.push('-');
            }
            pending_dash = false;
            sanitized.push(char::from(byte));
        } else {
            pending_dash = true;
        }
    }
    sanitized
}

/// Builds a stable workspace id from an executable file stem.
///
/// The result has the form `exe:<sanitized stem>`; stems that sanitise to
/// nothing map to the shared `exe:unknown` fallback.
///
/// ```
/// # use stickie::platform::workspace_id_from_exe_stem;
/// assert_eq!(workspace_id_from_exe_stem("EldenRing"), "exe:eldenring");
/// assert_eq!(workspace_id_from_exe_stem("dark souls 3"), "exe:dark-souls-3");
/// assert_eq!(workspace_id_from_exe_stem("***"), "exe:unknown");
/// ```
pub fn workspace_id_from_exe_stem(stem: &str) -> String {
    let sanitized = sanitize_id_component(stem);
    if sanitized.is_empty() {
        "exe:unknown".to_string()
    } else {
        format!("exe:{sanitized}")
    }
}

/// Returns the file stem of the last path component, e.g.
/// `C:\Games\ELDEN RING\eldenring.exe` -> `eldenring`.
///
/// Both `\` and `/` are accepted as separators so the helper can also be unit
/// tested with Linux-style paths.
pub fn exe_stem_from_path(path: &str) -> Option<&str> {
    let file_name = path
        .rsplit(['\\', '/'])
        .find(|component| !component.is_empty())?;
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    if stem.trim().is_empty() {
        None
    } else {
        Some(stem)
    }
}

/// Prettifies an executable stem into a readable fallback display name:
/// separators (`_`, `-`, `.`) become spaces and every word is capitalised.
/// ALL-CAPS stems are lowercased first so `ELDENRING` becomes `Eldenring`.
///
/// ```
/// # use stickie::platform::prettify_exe_stem;
/// assert_eq!(prettify_exe_stem("dark_souls_3"), "Dark Souls 3");
/// assert_eq!(prettify_exe_stem("ELDENRING"), "Eldenring");
/// assert_eq!(prettify_exe_stem("GTA5"), "Gta5");
/// ```
pub fn prettify_exe_stem(stem: &str) -> String {
    let mut display = String::with_capacity(stem.len());
    for word in stem
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        if !display.is_empty() {
            display.push(' ');
        }
        let all_caps = word.chars().any(|character| character.is_ascii_uppercase())
            && !word.chars().any(|character| character.is_ascii_lowercase());
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            display.push(first.to_ascii_uppercase());
            if all_caps {
                display.extend(characters.map(|character| character.to_ascii_lowercase()));
            } else {
                display.extend(characters);
            }
        }
    }
    display
}

/// Picks the display name for a game.
///
/// The foreground window title wins when it is present; otherwise the
/// executable stem is prettified. The result is always a non-empty, trimmed
/// string capped at [`MAX_DISPLAY_NAME_CHARS`] characters.
pub fn display_name_for(window_title: Option<&str>, exe_stem: Option<&str>) -> String {
    if let Some(title) = window_title.and_then(cap_window_title) {
        return title;
    }
    if let Some(stem) = exe_stem {
        let pretty = prettify_exe_stem(stem);
        if !pretty.is_empty() {
            return pretty;
        }
    }
    "Game".to_string()
}

/// Trims and caps a raw window title, keeping the first
/// [`MAX_DISPLAY_NAME_CHARS`] characters (with an ASCII ellipsis when cut).
fn cap_window_title(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= MAX_DISPLAY_NAME_CHARS {
        return Some(collapsed);
    }
    let mut capped: String = collapsed.chars().take(MAX_DISPLAY_NAME_CHARS - 3).collect();
    let trimmed = capped.trim_end().len();
    capped.truncate(trimmed);
    capped.push_str("...");
    Some(capped)
}

/// Returns `true` for executable names that belong to shell/desktop processes.
///
/// Both bare file names and full paths are accepted so callers can pass the
/// result of a process-image query directly.
pub fn is_shell_process(executable: &str) -> bool {
    let file_name = executable
        .rsplit(['\\', '/'])
        .find(|component| !component.is_empty())
        .unwrap_or(executable);
    SHELL_PROCESS_NAMES
        .iter()
        .any(|shell| file_name.eq_ignore_ascii_case(shell))
}

/// Returns `true` for window classes that belong to the Windows shell.
pub fn is_shell_window_class(class_name: &str) -> bool {
    SHELL_WINDOW_CLASSES
        .iter()
        .any(|class| class_name.eq_ignore_ascii_case(class))
}

/// Returns `true` when a foreground window is large enough to plausibly be a
/// game window instead of a toolbar, popup or splash helper.
pub fn is_game_like_window_size(width: i32, height: i32) -> bool {
    width >= MIN_GAME_WINDOW_WIDTH && height >= MIN_GAME_WINDOW_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_support_matches_target() {
        assert_eq!(overlay_supported(), cfg!(windows));
        assert_ne!(platform_name(), "");
    }

    #[test]
    fn sanitize_id_component_is_lowercase_and_dash_only() {
        assert_eq!(sanitize_id_component("Elden Ring"), "elden-ring");
        assert_eq!(sanitize_id_component("dark_souls_3"), "dark-souls-3");
        assert_eq!(sanitize_id_component("my.game"), "my-game");
        assert_eq!(sanitize_id_component("  --Hello--  "), "hello");
        assert_eq!(sanitize_id_component("***"), "");
        assert_eq!(sanitize_id_component(""), "");
    }

    #[test]
    fn sanitize_id_component_handles_non_ascii_as_separators() {
        assert_eq!(sanitize_id_component("Péché"), "p-ch");
        assert_eq!(sanitize_id_component("Æon"), "on");
    }

    #[test]
    fn workspace_ids_are_stable_for_common_names() {
        assert_eq!(workspace_id_from_exe_stem("EldenRing"), "exe:eldenring");
        assert_eq!(
            workspace_id_from_exe_stem("dark souls 3"),
            "exe:dark-souls-3"
        );
        assert_eq!(workspace_id_from_exe_stem("***"), "exe:unknown");
        // Stability: the id must not depend on the original casing.
        assert_eq!(
            workspace_id_from_exe_stem("ELDENRING"),
            workspace_id_from_exe_stem("eldenring")
        );
    }

    #[test]
    fn exe_stem_handles_windows_and_unix_paths() {
        assert_eq!(
            exe_stem_from_path(r"C:\Games\ELDEN RING\eldenring.exe"),
            Some("eldenring")
        );
        assert_eq!(exe_stem_from_path("/usr/bin/steam"), Some("steam"));
        assert_eq!(exe_stem_from_path(r"C:\Games\my.game.exe"), Some("my.game"));
        assert_eq!(exe_stem_from_path(""), None);
        assert_eq!(exe_stem_from_path("game.exe"), Some("game"));
    }

    #[test]
    fn prettify_makes_readable_names() {
        assert_eq!(prettify_exe_stem("eldenring"), "Eldenring");
        assert_eq!(prettify_exe_stem("ELDENRING"), "Eldenring");
        assert_eq!(prettify_exe_stem("dark_souls_3"), "Dark Souls 3");
        assert_eq!(prettify_exe_stem("Half-Life2"), "Half Life2");
        assert_eq!(prettify_exe_stem("***"), "");
    }

    #[test]
    fn display_name_prefers_trimmed_title() {
        assert_eq!(
            display_name_for(Some("  ELDEN RING   \t "), Some("eldenring")),
            "ELDEN RING"
        );
        assert_eq!(
            display_name_for(Some("Game -   Level 3"), None),
            "Game - Level 3"
        );
        assert_eq!(display_name_for(None, Some("dark_souls_3")), "Dark Souls 3");
        assert_eq!(display_name_for(Some("   "), Some("game")), "Game");
        assert_eq!(display_name_for(None, None), "Game");
    }

    #[test]
    fn display_name_is_capped() {
        let long = "A".repeat(200);
        let name = display_name_for(Some(&long), None);
        assert_eq!(name.chars().count(), MAX_DISPLAY_NAME_CHARS);
        assert!(name.ends_with("..."));
        // Short enough titles are never truncated.
        let short = "A".repeat(MAX_DISPLAY_NAME_CHARS);
        assert_eq!(display_name_for(Some(&short), None), short);
    }

    #[test]
    fn shell_processes_are_recognised_from_name_or_path() {
        assert!(is_shell_process("explorer.exe"));
        assert!(is_shell_process("EXPLORER.EXE"));
        assert!(is_shell_process(r"C:\Windows\explorer.exe"));
        assert!(is_shell_process(r"C:\Windows\SystemApps\SearchHost.exe"));
        assert!(!is_shell_process("eldenring.exe"));
        assert!(!is_shell_process(r"C:\Games\EldenRing\eldenring.exe"));
    }

    #[test]
    fn shell_window_classes_are_recognised() {
        assert!(is_shell_window_class("Progman"));
        assert!(is_shell_window_class("shell_traywnd"));
        assert!(!is_shell_window_class("UnityWndClass"));
        assert!(!is_shell_window_class("UnrealWindow"));
    }

    #[test]
    fn window_size_heuristic_rejects_toolbars() {
        assert!(is_game_like_window_size(1920, 1080));
        assert!(is_game_like_window_size(
            MIN_GAME_WINDOW_WIDTH,
            MIN_GAME_WINDOW_HEIGHT
        ));
        assert!(!is_game_like_window_size(1920, 200));
        assert!(!is_game_like_window_size(100, 100));
        assert!(!is_game_like_window_size(0, 0));
    }
}
