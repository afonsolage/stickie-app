//! Contract tests for the persistence store.
//!
//! Every test uses an isolated directory below the system temporary directory
//! and removes it on drop, so the suite is safe to run in parallel and leaves
//! nothing behind.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::{Rgba, RgbaImage};

use super::*;

/// Temporary directory that is deleted when the guard is dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!(
            "stickie-store-{label}-{}-{nanos}-{count}",
            process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Bootstraps a store with a workspace already open.
fn store_with_workspace(temp: &TempDir) -> Store {
    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    store
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("open workspace");
    store
}

#[test]
fn workspace_ids_are_sanitized() {
    assert_eq!(sanitize_workspace_id(""), "desktop");
    assert_eq!(sanitize_workspace_id("!!!"), "desktop");
    assert_eq!(sanitize_workspace_id("---"), "desktop");
    assert_eq!(sanitize_workspace_id("EXE:Elden Ring!!"), "exe-elden-ring");
    assert_eq!(sanitize_workspace_id("exe-eldenring"), "exe-eldenring");
    assert_eq!(sanitize_workspace_id("a__b  c"), "a-b-c");

    let long = "a".repeat(80);
    let sanitized = sanitize_workspace_id(&long);
    assert_eq!(sanitized.len(), 64);
    assert!(
        sanitized
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    );
}

#[test]
fn app_paths_create_layout_and_default_config() {
    let temp = TempDir::new("paths");
    let paths = AppPaths::at(temp.path());
    assert_eq!(paths.root(), temp.path());
    assert_eq!(paths.config_file(), temp.path().join("config.toml"));
    assert_eq!(paths.workspaces_dir(), temp.path().join("workspaces"));
    assert_eq!(paths.logs_dir(), temp.path().join("logs"));
    assert_eq!(
        paths.workspace_dir("EXE:Elden Ring!!"),
        temp.path().join("workspaces").join("exe-elden-ring")
    );

    let store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    assert_eq!(store.paths(), &paths);
    assert!(temp.path().join("workspaces").is_dir());
    assert!(temp.path().join("logs").is_dir());
    assert!(temp.path().join("config.toml").is_file());
    assert_eq!(store.config().toggle_hotkey, DEFAULT_TOGGLE_HOTKEY);
    assert_eq!(store.config().scrim_opacity, DEFAULT_SCRIM_OPACITY);
    assert!(store.config().start_hidden);
    assert_eq!(store.config().last_workspace, None);
    assert!(store.workspace().is_none());
}

#[test]
fn config_round_trips_and_clamps_opacity() {
    let temp = TempDir::new("config");
    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    store
        .save_config(Config {
            toggle_hotkey: "  Ctrl+Alt+N  ".to_string(),
            scrim_opacity: 2.0,
            start_hidden: false,
            last_workspace: Some("EXE:Elden Ring!!".to_string()),
        })
        .expect("save config");

    assert_eq!(store.config().toggle_hotkey, "Ctrl+Alt+N");
    assert_eq!(store.config().scrim_opacity, MAX_SCRIM_OPACITY);
    assert!(!store.config().start_hidden);
    assert_eq!(
        store.config().last_workspace.as_deref(),
        Some("exe-elden-ring")
    );

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    assert_eq!(reopened.config().toggle_hotkey, "Ctrl+Alt+N");
    assert_eq!(reopened.config().scrim_opacity, MAX_SCRIM_OPACITY);
    assert!(!reopened.config().start_hidden);
    assert_eq!(
        reopened.config().last_workspace.as_deref(),
        Some("exe-elden-ring")
    );

    reopened
        .save_config(Config {
            toggle_hotkey: "   ".to_string(),
            scrim_opacity: -3.0,
            ..Config::default()
        })
        .expect("save normalised config");
    assert_eq!(reopened.config().toggle_hotkey, DEFAULT_TOGGLE_HOTKEY);
    assert_eq!(reopened.config().scrim_opacity, 0.0);
}

#[test]
fn corrupt_config_is_backed_up_and_defaults_are_used() {
    let temp = TempDir::new("corrupt-config");
    fs::create_dir_all(temp.path()).expect("create data root");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "toggle_hotkey = [this is not toml").expect("write corrupt config");

    let store = Store::bootstrap_at(temp.path()).expect("bootstrap recovers");
    assert_eq!(store.config().toggle_hotkey, DEFAULT_TOGGLE_HOTKEY);
    assert_eq!(store.config().scrim_opacity, DEFAULT_SCRIM_OPACITY);
    assert!(store.config().start_hidden);

    let backup_path = temp.path().join("config.toml.bak");
    assert!(backup_path.is_file(), "corrupt config should be backed up");
    let backup = fs::read_to_string(&backup_path).expect("read backup");
    assert!(backup.contains("this is not toml"));
    // A fresh, valid configuration is written after recovery.
    assert!(config_path.is_file());
}

#[test]
fn corrupt_workspace_manifest_is_backed_up() {
    let temp = TempDir::new("corrupt-workspace");
    {
        let mut store = store_with_workspace(&temp);
        store
            .create_sticky()
            .expect("sticky so the manifest has content");
    }
    let manifest_path = temp
        .path()
        .join("workspaces")
        .join("exe-testgame")
        .join("workspace.toml");
    assert!(manifest_path.is_file());
    fs::write(&manifest_path, "[[[ definitely not toml").expect("corrupt manifest");

    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    let workspace = store
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("recover workspace");
    assert!(workspace.stickies().is_empty());
    assert!(manifest_path.is_file(), "a fresh manifest is written");
    let backup_path = temp
        .path()
        .join("workspaces")
        .join("exe-testgame")
        .join("workspace.toml.bak");
    assert!(
        backup_path.is_file(),
        "corrupt manifest should be backed up"
    );
    let backup = fs::read_to_string(&backup_path).expect("read manifest backup");
    assert!(backup.contains("definitely not toml"));
}

#[test]
fn workspace_open_reopen_restores_last_folder() {
    let temp = TempDir::new("workspace");
    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    let descriptor = WorkspaceDescriptor::new("EXE:Elden Ring!!", "Elden Ring")
        .with_executable(Some("eldenring.exe"))
        .with_window_title(Some("ELDEN RING"));

    {
        let workspace = store.open_workspace(descriptor).expect("open workspace");
        assert_eq!(workspace.id(), "exe-elden-ring");
        assert_eq!(workspace.display_name(), "Elden Ring");
        assert_eq!(workspace.executable(), Some("eldenring.exe"));
        assert_eq!(workspace.window_title(), Some("ELDEN RING"));
        assert!(workspace.root().is_dir());
        assert!(workspace.notes_dir().is_dir());
        assert!(workspace.screenshots_dir().is_dir());
        assert!(workspace.files_dir().is_dir());
        assert!(workspace.root().join("workspace.toml").is_file());
        assert!(workspace.stickies().is_empty());
        assert!(workspace.last_folder().is_none());
        assert!(workspace.created_at() > 0);
    }
    assert_eq!(
        store.config().last_workspace.as_deref(),
        Some("exe-elden-ring")
    );
    assert_eq!(
        store.notes_dir(),
        Some(store.workspace().unwrap().notes_dir())
    );
    assert_eq!(store.last_folder(), None);
    store
        .set_last_folder("saves\\current")
        .expect("set last folder");
    assert_eq!(
        store.last_folder(),
        Some("saves/current"),
        "manifest paths are stored with forward slashes"
    );
    let created_at = store.workspace().expect("workspace open").created_at();
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    assert_eq!(
        reopened.config().last_workspace.as_deref(),
        Some("exe-elden-ring")
    );
    let workspace = reopened
        .open_workspace(WorkspaceDescriptor::new(
            "exe-elden-ring",
            "Elden Ring (reopened)",
        ))
        .expect("reopen workspace");
    assert_eq!(workspace.last_folder(), Some("saves/current"));
    assert_eq!(workspace.created_at(), created_at);
    // Game metadata is refreshed on every open while workspace data is kept.
    assert_eq!(workspace.display_name(), "Elden Ring (reopened)");
}

#[test]
fn notes_create_write_read_rename_delete_and_sort() {
    let temp = TempDir::new("notes");
    let mut store = store_with_workspace(&temp);

    let first = store.create_note().expect("create first note");
    let second = store.create_note().expect("create second note");
    assert_eq!(first.title, "Untitled");
    assert_eq!(first.relative, PathBuf::from("Untitled.md"));
    assert_eq!(second.title, "Untitled 2");
    assert_eq!(second.relative, PathBuf::from("Untitled 2.md"));
    assert!(first.path.is_file());
    assert!(second.path.is_file());

    // Notes are plain markdown files, so external editors can create them too.
    let notes_dir = store.notes_dir().expect("notes dir").to_path_buf();
    fs::write(notes_dir.join("External.md"), "created externally").expect("write external note");

    let contents = "# First note\n\nline one\nline two\n";
    store.write_note(&first, contents).expect("write note");
    assert_eq!(store.read_note(&first).expect("read note"), contents);

    // Make deterministic ordering possible without a filetime dependency.
    std::thread::sleep(Duration::from_millis(1_100));
    store
        .write_note(&second, "second note, newest")
        .expect("write second note");

    let notes = store.list_notes().expect("list notes");
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[0].title, "Untitled 2", "newest note first");
    let titles: Vec<&str> = notes.iter().map(|note| note.title.as_str()).collect();
    assert!(titles.contains(&"Untitled"));
    assert!(titles.contains(&"External"));
    assert!(
        notes
            .windows(2)
            .all(|pair| pair[0].modified_secs >= pair[1].modified_secs)
    );

    let renamed = store
        .rename_note(&first, "Shopping List")
        .expect("rename note");
    assert_eq!(renamed.title, "Shopping List");
    assert_eq!(renamed.relative, PathBuf::from("Shopping List.md"));
    assert!(renamed.path.is_file());
    assert!(!first.path.exists());

    store.delete_note(&second).expect("delete note");
    store
        .delete_note(&second)
        .expect("deleting a missing note is not an error");

    let remaining: Vec<String> = store
        .list_notes()
        .expect("list notes")
        .into_iter()
        .map(|note| note.title)
        .collect();
    assert_eq!(remaining.len(), 2);
    assert!(remaining.contains(&"Shopping List".to_string()));
    assert!(remaining.contains(&"External".to_string()));
}

#[test]
fn folders_are_listed_directories_first_and_sorted() {
    let temp = TempDir::new("folders");
    let mut store = store_with_workspace(&temp);
    let files_dir = store.files_dir().expect("files dir").to_path_buf();
    fs::write(files_dir.join("b.txt"), "b").expect("write file b");
    fs::write(files_dir.join("A.txt"), "a").expect("write file a");

    store.create_folder("", "Beta").expect("create Beta");
    store.create_folder("", "alpha").expect("create alpha");
    let nested = store
        .create_folder("alpha", "zeta")
        .expect("create nested folder");
    assert_eq!(nested.relative, PathBuf::from("alpha").join("zeta"));
    assert!(nested.is_dir);
    assert!(nested.path.is_dir());

    let listing = store.list_folder("").expect("list root folder");
    let names: Vec<&str> = listing.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["alpha", "Beta", "A.txt", "b.txt"]);
    assert!(listing[0].is_dir);
    assert!(listing[1].is_dir);
    assert!(!listing[2].is_dir);
    assert!(!listing[3].is_dir);

    let alpha = store.list_folder("alpha").expect("list alpha");
    assert_eq!(alpha.len(), 1);
    assert_eq!(alpha[0].name, "zeta");

    let deeper = store
        .create_folder("alpha/zeta", "inner")
        .expect("create deeper folder");
    assert_eq!(deeper.relative, PathBuf::from("alpha/zeta/inner"));
    assert_eq!(
        store.list_folder("alpha/zeta").expect("list deeper").len(),
        1
    );

    assert!(
        store.list_folder("does-not-exist").is_err(),
        "listing a missing folder should surface an error"
    );
}

#[test]
fn folder_paths_reject_traversal() {
    let temp = TempDir::new("traversal");
    let mut store = store_with_workspace(&temp);

    for bad in [
        "..",
        "../outside",
        "a/../../outside",
        "/absolute",
        "\\\\server\\share",
        "C:\\Windows",
        "a\\..\\..\\b",
    ] {
        assert!(
            store.list_folder(bad).is_err(),
            "list_folder should reject {bad:?}"
        );
    }

    assert!(store.create_folder("..", "escape").is_err());
    assert!(store.create_folder("", "..").is_err());
    assert!(store.create_folder("", "nested/name").is_err());
    assert!(store.create_folder("", "C:").is_err());
    assert!(store.set_last_folder("../escape").is_err());
    assert_eq!(store.last_folder(), None);

    let escape = temp.path().join("escape");
    assert!(
        !escape.exists(),
        "no traversal escape file should be created"
    );
}

#[cfg(unix)]
#[test]
fn folder_paths_reject_symlink_escapes() {
    let temp = TempDir::new("symlink");
    let outside = TempDir::new("symlink-outside");
    let mut store = store_with_workspace(&temp);
    let link = store.files_dir().expect("files dir").join("escape");
    std::os::unix::fs::symlink(outside.path(), &link).expect("create escaping symlink");

    assert!(store.list_folder("escape").is_err());
    assert!(store.create_folder("escape", "blocked").is_err());
}

#[test]
fn stickies_persist_across_reopen() {
    let temp = TempDir::new("stickies");
    let mut store = store_with_workspace(&temp);

    let mut sticky = store.create_sticky().expect("create sticky");
    assert!(!sticky.id.is_empty());
    assert_eq!(sticky.color, StickyColor::default());
    assert_eq!(StickyColor::default(), StickyColor::Amber);

    sticky.text = "line one\nline two\n\nline four".to_string();
    sticky.color = StickyColor::Rose;
    sticky.x = 12.5;
    sticky.y = -3.0;
    sticky.width = 300.0;
    sticky.height = 200.0;
    store.save_sticky(sticky.clone()).expect("save sticky note");
    assert_eq!(store.stickies().len(), 1);
    assert_eq!(store.stickies()[0].text, sticky.text);

    let second = store.create_sticky().expect("create second sticky");
    assert_ne!(second.id, sticky.id);
    store
        .delete_sticky(&second.id)
        .expect("delete second sticky");
    store
        .delete_sticky("missing-sticky")
        .expect("deleting a missing sticky is not an error");
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace");
    let stickies = reopened.stickies();
    assert_eq!(stickies.len(), 1);
    assert_eq!(stickies[0].id, sticky.id);
    assert_eq!(stickies[0].text, "line one\nline two\n\nline four");
    assert_eq!(stickies[0].color, StickyColor::Rose);
    assert!((stickies[0].x - 12.5).abs() < f32::EPSILON);
    assert!((stickies[0].y - -3.0).abs() < f32::EPSILON);
    assert!((stickies[0].width - 300.0).abs() < f32::EPSILON);
    assert!(stickies[0].created_at > 0);
    assert!(stickies[0].updated_at >= stickies[0].created_at);

    // Saving an existing id updates in place and keeps the creation timestamp.
    let mut updated = stickies[0].clone();
    updated.text = "updated text".to_string();
    updated.created_at = 0;
    reopened.save_sticky(updated).expect("update sticky note");
    assert_eq!(reopened.stickies().len(), 1);
    assert_eq!(reopened.stickies()[0].text, "updated text");
    assert_eq!(reopened.stickies()[0].created_at, sticky.created_at);

    reopened
        .delete_sticky(&sticky.id)
        .expect("delete last sticky");
    assert!(reopened.stickies().is_empty());
}

#[test]
fn screenshots_use_unique_names_and_list_newest_first() {
    let temp = TempDir::new("screenshots");
    let store = store_with_workspace(&temp);
    let image = RgbaImage::from_pixel(4, 3, Rgba([120, 30, 200, 255]));

    let first = store
        .save_screenshot(&image)
        .expect("save first screenshot");
    let second = store
        .save_screenshot(&image)
        .expect("save second screenshot");
    assert_ne!(first, second);
    assert!(first.is_file());
    assert!(second.is_file());

    let screenshots_dir = store
        .screenshots_dir()
        .expect("screenshots dir")
        .to_path_buf();
    assert!(first.starts_with(&screenshots_dir));
    assert!(second.starts_with(&screenshots_dir));
    for path in [&first, &second] {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        assert!(
            name.ends_with("-screenshot.png"),
            "unexpected screenshot name {name:?}"
        );
    }

    let listed = store.list_screenshots().expect("list screenshots");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0], second, "newest screenshot first");
    assert_eq!(listed[1], first);

    let saved = image::open(&first).expect("open saved screenshot");
    assert_eq!((saved.width(), saved.height()), (4, 3));
}
