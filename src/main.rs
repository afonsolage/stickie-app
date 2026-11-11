// Hide the console window in release Windows builds. Debug builds keep stdout
// and eprintln! visible, which is invaluable while developing the overlay.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;

fn main() {
    if let Err(error) = stickie::run() {
        report_fatal_error(&error);
        std::process::exit(1);
    }
}

/// A GUI-subsystem Windows app has no console, so make startup failures
/// discoverable: print when possible and always attempt a small log file next
/// to the app data.
fn report_fatal_error(error: &anyhow::Error) {
    eprintln!("Stickie failed to start: {error:#}");
    let mut candidates = Vec::new();
    if let Some(data_dir) = std::env::var_os("STICKIE_DATA_DIR") {
        candidates.push(PathBuf::from(data_dir));
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(app_data).join("Stickie"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/share/Stickie"));
    }
    for directory in candidates {
        if std::fs::create_dir_all(&directory).is_ok() {
            let path = directory.join("startup-error.log");
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let message = format!("[{stamp}] Stickie failed to start:\n{error:#}\n");
            if std::fs::write(&path, message).is_ok() {
                eprintln!("Wrote startup error to {}", path.display());
                return;
            }
        }
    }
}
