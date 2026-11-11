//! Win32 implementation of the [`crate::platform`] facade.
//!
//! Compiled only on Windows. Everything here is deliberately conservative: the
//! functions run once per overlay open, never panic on partial failures and
//! degrade to [`None`]/an error when a call fails. That matters because games
//! with anti-cheat or elevated processes often refuse process queries.
//!
//! # Safety
//!
//! This module wraps raw Win32 calls, so it is the only part of the crate
//! (together with the Windows hotkey registration) that allows `unsafe_code`.
//! Every `unsafe` block carries a SAFETY comment and every Win32 failure is
//! checked instead of assumed away.

#![allow(unsafe_code)]
// Win32 exposes pointer-sized and 32-bit integer types; the conversions below
// are bounded by the fixed buffer sizes declared in this module.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use image::RgbaImage;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows_sys::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SMTO_ABORTIFHUNG, SMTO_BLOCK, SW_RESTORE, SendMessageTimeoutW,
    SetForegroundWindow, ShowWindow, WM_GETTEXT, WM_GETTEXTLENGTH, WS_EX_TOOLWINDOW,
};

use super::{
    GameContext, display_name_for, exe_stem_from_path, is_game_like_window_size, is_shell_process,
    is_shell_window_class, workspace_id_from_exe_stem,
};

/// Upper bound for a Windows process image path (long paths are up to 32767
/// UTF-16 code units).
const MAX_PROCESS_IMAGE_CHARS: usize = 32_768;

/// Upper bound for a window title captured from the foreground window. Titles
/// longer than this are truncated; the display name is capped far lower anyway.
const MAX_WINDOW_TITLE_CHARS: usize = 512;

/// Upper bound for a Win32 window class name.
const MAX_WINDOW_CLASS_CHARS: usize = 256;

/// Probes the foreground window and turns it into a [`GameContext`].
///
/// Returns [`None`] when there is no foreground window, when it is hidden,
/// minimized, too small or owned by the Windows shell, when the owning process
/// is Stickie itself, or when the executable path of the owning process cannot
/// be queried (without it there is no stable workspace id).
pub fn active_game() -> Option<GameContext> {
    // SAFETY: `GetForegroundWindow` takes no arguments and returns either a
    // valid window handle or NULL; the handle is validated with `IsWindow`
    // before any other call uses it.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }

    // SAFETY: `window` is a non-null raw HWND returned by the OS. All four
    // queries are safe for any handle and report failure through their return
    // value instead of panicking; `GetShellWindow`/`GetDesktopWindow` return
    // well-known shell windows.
    let shell_window = unsafe { GetShellWindow() };
    if window == shell_window {
        return None;
    }
    // SAFETY: see above.
    if window == unsafe { GetDesktopWindow() } {
        return None;
    }

    // SAFETY: `window` is a non-null HWND. `IsWindow`, `IsWindowVisible` and
    // `IsIconic` are pure queries that accept any handle.
    let is_window = unsafe { IsWindow(window) } != 0;
    // SAFETY: see above.
    let is_visible = unsafe { IsWindowVisible(window) } != 0;
    // SAFETY: see above.
    let is_minimized = unsafe { IsIconic(window) } != 0;
    if !is_window || !is_visible || is_minimized {
        return None;
    }

    // Tool windows are tray icons, tooltips and assorted shell popups.
    if is_tool_window(window) {
        return None;
    }

    if !window_is_large_enough(window) {
        return None;
    }

    // The desktop/taskbar is hosted by explorer's special windows; their class
    // names are stable across Windows versions.
    if let Some(class_name) = read_window_class(window)
        && is_shell_window_class(&class_name)
    {
        return None;
    }

    let window_title = read_window_title(window);

    let mut process_id = 0u32;
    // SAFETY: `window` is a valid HWND and `process_id` is a valid out-pointer
    // to a `u32`; the call writes the process id for the window's thread.
    let thread_id = unsafe { GetWindowThreadProcessId(window, &raw mut process_id) };
    if thread_id == 0 || process_id == 0 {
        return None;
    }
    // Never select Stickie for a workspace: probing runs just before the
    // overlay becomes visible, while the previous foreground window may still
    // be one of our own windows.
    if process_id == std::process::id() {
        return None;
    }

    let executable = process_image_path(process_id)?;
    if is_own_executable(&executable) {
        return None;
    }
    // Desktop, Start menu, search and taskbar windows are ordinary windows in
    // shell processes; skip them so no workspace is created for the desktop.
    if is_shell_process(&executable) {
        return None;
    }

    let exe_stem = exe_stem_from_path(&executable)?;
    Some(GameContext {
        workspace_id: workspace_id_from_exe_stem(exe_stem),
        display_name: display_name_for(window_title.as_deref(), Some(exe_stem)),
        executable: Some(executable),
        window_title,
        process_id: Some(process_id),
        window_handle: Some(window as isize),
    })
}

/// Restores the foreground focus to the game window captured in `game`.
///
/// Best effort: Windows may refuse foreground changes (`SetForegroundWindow`
/// returns `FALSE`) when another process owns the foreground lock. Failures are
/// logged and ignored; the overlay is already hidden at this point.
pub fn restore_game_focus(game: &GameContext) {
    let Some(handle) = game.window_handle else {
        return;
    };
    if handle == 0 {
        return;
    }
    let window = handle as HWND;

    // SAFETY: `window` was captured from `GameContext` created by
    // `active_game`; `IsWindow` validates it before any call that would touch
    // the window, and Win32 reports failures through BOOL return values, so no
    // call can panic.
    unsafe {
        if IsWindow(window) == 0 {
            log::debug!("cannot restore focus: game window handle {handle} is no longer valid");
            return;
        }
        if IsIconic(window) != 0 {
            // A minimized game has to be restored before it can take focus.
            let _ = ShowWindow(window, SW_RESTORE);
        }
        if SetForegroundWindow(window) == 0 {
            log::warn!(
                "SetForegroundWindow failed for game window {handle}: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// Captures a screenshot of the game window, falling back to the primary
/// monitor when the game window cannot be captured.
///
/// When `game` carries a process id, the largest (preferring focused) visible
/// xcap window of that process is captured. In every other case - no context,
/// no process id, a capture error or a context that points at Stickie itself -
/// the primary monitor is captured instead.
pub fn capture_game(game: Option<&GameContext>) -> Result<RgbaImage> {
    // Windows Graphics Capture uses WinRT, which requires the calling thread to
    // be in a COM apartment. The app runs capture on a dedicated worker thread,
    // so initialise it there; RPC_E_CHANGED_MODE (already initialised in another
    // apartment) is harmless for this one-shot capture.
    // SAFETY: no out-pointers are passed; `CoInitializeEx` is safe to call once
    // per thread and its HRESULT is intentionally ignored for the reason above.
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
    }

    if let Some(game) = game
        && let Some(process_id) = game.process_id
    {
        if process_id == std::process::id() {
            // Defensive: never screenshot our own (transparent) overlay.
            log::debug!("refusing to capture Stickie's own process; using the primary monitor");
        } else {
            match capture_window_for_process(process_id) {
                Ok(image) => return Ok(image),
                Err(error) => log::warn!(
                    "could not capture the game window for process {process_id}: {error:#}; \
                     falling back to the primary monitor"
                ),
            }
        }
    }
    capture_primary_monitor()
}

/// Finds the best visible xcap window for `process_id` and captures it.
///
/// "Best" means focused first, then largest by pixel area. Stickie's own
/// process is filtered out defensively even though callers already skip it.
fn capture_window_for_process(process_id: u32) -> Result<RgbaImage> {
    let windows = xcap::Window::all()
        .map_err(|error| anyhow!("could not enumerate desktop windows: {error}"))?;
    let own_process_id = std::process::id();

    let mut best: Option<(bool, u64, xcap::Window)> = None;
    for window in windows {
        let Ok(window_process_id) = window.pid() else {
            continue;
        };
        if window_process_id != process_id || window_process_id == own_process_id {
            continue;
        }
        // An unknown minimized state is treated as minimized: capturing it
        // would produce stale or black pixels.
        if window.is_minimized().unwrap_or(true) {
            continue;
        }
        let width = window.width().unwrap_or(0);
        let height = window.height().unwrap_or(0);
        if width == 0 || height == 0 {
            continue;
        }
        let focused = window.is_focused().unwrap_or(false);
        let area = u64::from(width) * u64::from(height);
        let is_better = match best {
            None => true,
            Some((best_focused, best_area, _)) => {
                (focused && !best_focused) || (focused == best_focused && area > best_area)
            }
        };
        if is_better {
            best = Some((focused, area, window));
        }
    }

    let Some((_, _, window)) = best else {
        bail!("no visible window found for process id {process_id}");
    };
    let image = window
        .capture_image()
        .map_err(|error| anyhow!("xcap window capture failed: {error}"))?;
    log::debug!(
        "captured game window {}x{} for process {}",
        image.width(),
        image.height(),
        process_id
    );
    Ok(image)
}

/// Captures the primary monitor (or the first monitor when none is flagged as
/// primary).
fn capture_primary_monitor() -> Result<RgbaImage> {
    let monitors =
        xcap::Monitor::all().map_err(|error| anyhow!("could not enumerate monitors: {error}"))?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow!("no monitors are available for capture"))?;
    let image = monitor
        .capture_image()
        .map_err(|error| anyhow!("xcap monitor capture failed: {error}"))?;
    log::debug!(
        "captured primary monitor {}x{}",
        image.width(),
        image.height()
    );
    Ok(image)
}

/// Opens `path` in Explorer. Directories are opened directly; files open their
/// containing folder.
pub fn open_in_file_manager(path: &Path) -> Result<()> {
    ensure_exists(path)?;
    let folder = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
    };
    launch_explorer(&[folder.as_os_str()])
}

/// Reveals `path` in Explorer: files are selected in their containing folder,
/// directories are opened directly (Explorer has no portable "select this
/// folder" command line).
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    ensure_exists(path)?;
    if path.is_dir() {
        return open_in_file_manager(path);
    }
    let select_argument = format!("/select,{}", path.display());
    launch_explorer(&[OsStr::new(&select_argument)])
}

fn ensure_exists(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("cannot open an empty path");
    }
    if !path.exists() {
        bail!("path does not exist: {}", path.display());
    }
    Ok(())
}

fn launch_explorer(arguments: &[&OsStr]) -> Result<()> {
    let mut command = Command::new("explorer.exe");
    command.args(arguments);
    command
        .spawn()
        .map(|_child| ())
        .with_context(|| format!("failed to launch explorer.exe with {arguments:?}"))?;
    Ok(())
}

/// Returns `true` when the window has the `WS_EX_TOOLWINDOW` extended style.
fn is_tool_window(window: HWND) -> bool {
    // SAFETY: `GetWindowLongPtrW` only reads a window style and returns 0 on
    // failure for a handle that became invalid between the checks above.
    let extended_style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    let extended_style = u32::try_from(extended_style).unwrap_or(0);
    (extended_style & WS_EX_TOOLWINDOW) != 0
}

/// Returns `true` when the window rectangle meets the game-like size heuristic.
fn window_is_large_enough(window: HWND) -> bool {
    let mut rectangle = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `window` is a non-null HWND validated by `IsWindow`; `rectangle`
    // is a valid out-pointer to a stack `RECT`.
    if unsafe { GetWindowRect(window, &raw mut rectangle) } == 0 {
        log::debug!(
            "could not query the foreground window rectangle: {}",
            std::io::Error::last_os_error()
        );
        return false;
    }
    let width = rectangle.right.saturating_sub(rectangle.left);
    let height = rectangle.bottom.saturating_sub(rectangle.top);
    is_game_like_window_size(width, height)
}

/// Reads the window title. Returns [`None`] for empty titles and on failure.
fn read_window_title(window: HWND) -> Option<String> {
    // Cross-process `GetWindowTextW` sends WM_GETTEXT synchronously, which can
    // block forever when the foreground application has a hung UI thread (or is
    // slow to start). SendMessageTimeout + SMTO_ABORTIFHUNG keeps the overlay
    // responsive: the title is optional metadata, never worth a hang.
    const TIMEOUT_MS: u32 = 50;
    let flags = SMTO_ABORTIFHUNG | SMTO_BLOCK;
    let mut result: usize = 0;

    // SAFETY: `window` is a valid HWND. The result pointer is valid for the
    // duration of the call, and the timeout/abort flag prevents a hung target
    // from blocking the game probe indefinitely.
    let length_ok = unsafe {
        SendMessageTimeoutW(
            window,
            WM_GETTEXTLENGTH,
            0,
            0,
            flags,
            TIMEOUT_MS,
            &raw mut result,
        )
    };
    if length_ok == 0 || result == 0 {
        return None;
    }

    let capacity = (result + 1).min(MAX_WINDOW_TITLE_CHARS);
    let mut buffer = vec![0u16; capacity];
    // SAFETY: same window; `buffer` is exclusively owned and writable for
    // `capacity` UTF-16 units. WM_GETTEXT copies at most wParam characters and
    // null-terminates the buffer.
    let copied_ok = unsafe {
        SendMessageTimeoutW(
            window,
            WM_GETTEXT,
            capacity,
            buffer.as_mut_ptr().cast::<core::ffi::c_void>() as isize,
            flags,
            TIMEOUT_MS,
            &raw mut result,
        )
    };
    if copied_ok == 0 {
        return None;
    }
    let copied = result.min(capacity.saturating_sub(1));
    if copied == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..copied]))
}

/// Reads the Win32 class name of the window. Returns [`None`] on failure.
fn read_window_class(window: HWND) -> Option<String> {
    let mut buffer = [0u16; MAX_WINDOW_CLASS_CHARS];
    // SAFETY: `window` is valid and `buffer` holds `buffer.len()` UTF-16 code
    // units; `GetClassNameW` writes at most that many units.
    let copied = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    if copied <= 0 {
        return None;
    }
    let copied = (usize::try_from(copied).unwrap_or(0)).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..copied]))
}

/// Queries the full executable path of a process.
///
/// Returns [`None`] when the process cannot be opened (protected/elevated
/// process, process exited between the two calls, ...) or the path query fails.
fn process_image_path(process_id: u32) -> Option<String> {
    // SAFETY: `OpenProcess` takes a process id obtained from a live window and
    // only requests query rights; the returned handle is checked for NULL and
    // closed exactly once below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        log::debug!(
            "cannot open process {process_id} to read its image path: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }

    let mut buffer = vec![0u16; MAX_PROCESS_IMAGE_CHARS];
    let mut length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: `process` is a valid process handle; `buffer` is allocated for
    // `length` UTF-16 code units and `length` is updated by the call to the
    // number of units actually written (always <= `buffer.len()`).
    let succeeded = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            buffer.as_mut_ptr(),
            &raw mut length,
        )
    };
    // SAFETY: `process` was returned by `OpenProcess` and is valid here, so it
    // must be closed exactly once.
    let _ = unsafe { CloseHandle(process) };

    if succeeded == 0 {
        log::debug!(
            "QueryFullProcessImageNameW failed for process {process_id}: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }
    let length = (usize::try_from(length).unwrap_or(0)).min(buffer.len());
    if length == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Returns `true` when `path` is the currently running Stickie executable.
///
/// The process-id check in [`active_game`] already filters us out; this is a
/// second, defence-in-depth check that also covers unusual launchers that
/// re-use our image under another pid.
fn is_own_executable(path: &str) -> bool {
    std::env::current_exe()
        .is_ok_and(|current| current.to_string_lossy().eq_ignore_ascii_case(path))
}
