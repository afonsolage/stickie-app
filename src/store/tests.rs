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
            .map_or(0, |duration| duration.as_nanos());
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

/// Writes a raw `workspace.toml` for `exe-testgame` before the workspace opens.
fn write_raw_workspace_manifest(temp: &TempDir, body: &str) {
    let directory = temp.path().join("workspaces").join("exe-testgame");
    fs::create_dir_all(&directory).expect("create raw workspace directory");
    fs::write(directory.join("workspace.toml"), body).expect("write raw workspace manifest");
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
    assert_eq!(workspace.stickies(), []);
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
        assert_eq!(workspace.stickies(), []);
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
    assert_ne!(sticky.id, "");
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
    assert_eq!(reopened.stickies(), []);
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

#[test]
fn pinned_stickies_persist_across_reopen() {
    let temp = TempDir::new("pinned");
    let mut store = store_with_workspace(&temp);
    let sticky = store.create_sticky().expect("create sticky");
    let other = store.create_sticky().expect("create second sticky");

    assert!(!store.is_sticky_pinned(&sticky.id));
    assert_eq!(store.pinned_stickies(), []);

    store
        .pin_sticky(&sticky.id, 120.0, -40.0, 320.0, 260.0)
        .expect("pin sticky");
    assert!(store.is_sticky_pinned(&sticky.id));
    assert!(!store.is_sticky_pinned(&other.id));

    let pinned = store.pinned_stickies().to_vec();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].sticky_id, sticky.id);
    assert!((pinned[0].x - 120.0).abs() < f32::EPSILON);
    assert!((pinned[0].y - -40.0).abs() < f32::EPSILON);
    assert!((pinned[0].width - 320.0).abs() < f32::EPSILON);
    assert!((pinned[0].height - 260.0).abs() < f32::EPSILON);
    // The workspace getter mirrors the store facade.
    assert_eq!(
        store.workspace().expect("workspace open").pinned_stickies(),
        pinned.as_slice()
    );
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace");
    assert_eq!(reopened.pinned_stickies(), pinned.as_slice());
    assert!(reopened.is_sticky_pinned(&sticky.id));

    reopened.unpin_sticky(&sticky.id).expect("unpin sticky");
    assert!(!reopened.is_sticky_pinned(&sticky.id));
    assert_eq!(reopened.pinned_stickies(), []);
    reopened
        .unpin_sticky(&sticky.id)
        .expect("unpinning twice is a no-op");
    drop(reopened);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store again");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace again");
    assert_eq!(reopened.pinned_stickies(), []);
    assert_eq!(reopened.stickies().len(), 2);
}

#[test]
fn old_manifest_without_pinned_stickies_loads() {
    let temp = TempDir::new("old-manifest");
    write_raw_workspace_manifest(
        &temp,
        r#"version = 1
id = "exe-testgame"
display_name = "Test Game"
created_at = 100
updated_at = 100

[[stickies]]
id = "sticky-old"
text = "legacy sticky"
color = "amber"
x = 10.0
y = 20.0
width = 240.0
height = 180.0
created_at = 100
updated_at = 100
"#,
    );

    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    let workspace = store
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("open legacy workspace");
    assert_eq!(workspace.stickies().len(), 1);
    assert_eq!(workspace.stickies()[0].id, "sticky-old");
    assert_eq!(workspace.stickies()[0].text, "legacy sticky");
    assert_eq!(workspace.pinned_stickies(), []);
    assert_eq!(store.pinned_stickies(), []);
    assert!(!store.is_sticky_pinned("sticky-old"));
}

#[test]
fn pinning_twice_updates_geometry_instead_of_duplicating() {
    let temp = TempDir::new("pin-twice");
    let mut store = store_with_workspace(&temp);
    let sticky = store.create_sticky().expect("create sticky");

    store
        .pin_sticky(&sticky.id, 10.0, 20.0, 200.0, 150.0)
        .expect("first pin");
    store
        .pin_sticky(&sticky.id, 300.0, 400.0, 500.0, 350.0)
        .expect("second pin updates geometry");
    let pinned = store.pinned_stickies();
    assert_eq!(pinned.len(), 1, "re-pinning must not duplicate the entry");
    assert!((pinned[0].x - 300.0).abs() < f32::EPSILON);
    assert!((pinned[0].y - 400.0).abs() < f32::EPSILON);
    assert!((pinned[0].width - 500.0).abs() < f32::EPSILON);
    assert!((pinned[0].height - 350.0).abs() < f32::EPSILON);

    // Re-pinning also clamps out-of-range geometry.
    store
        .pin_sticky(&sticky.id, 999.0, 999.0, 1.0, 1.0)
        .expect("third pin clamps geometry");
    let pinned = store.pinned_stickies();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].width, PINNED_STICKY_MIN_WIDTH);
    assert_eq!(pinned[0].height, PINNED_STICKY_MIN_HEIGHT);
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace");
    let pinned = reopened.pinned_stickies();
    assert_eq!(pinned.len(), 1);
    assert!((pinned[0].x - 999.0).abs() < f32::EPSILON);
    assert_eq!(pinned[0].width, PINNED_STICKY_MIN_WIDTH);
}

#[test]
fn deleting_a_sticky_removes_its_pin() {
    let temp = TempDir::new("delete-pinned");
    let mut store = store_with_workspace(&temp);
    let doomed = store.create_sticky().expect("create sticky");
    let kept = store.create_sticky().expect("create second sticky");
    store
        .pin_sticky(&doomed.id, 10.0, 20.0, 240.0, 180.0)
        .expect("pin doomed sticky");
    store
        .pin_sticky(&kept.id, 30.0, 40.0, 260.0, 200.0)
        .expect("pin kept sticky");

    store.delete_sticky(&doomed.id).expect("delete sticky");
    assert!(!store.is_sticky_pinned(&doomed.id));
    assert!(store.is_sticky_pinned(&kept.id));
    assert_eq!(store.pinned_stickies().len(), 1);
    assert_eq!(store.pinned_stickies()[0].sticky_id, kept.id);
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace");
    assert_eq!(reopened.stickies().len(), 1);
    assert_eq!(reopened.pinned_stickies().len(), 1);
    assert_eq!(reopened.pinned_stickies()[0].sticky_id, kept.id);
}

#[test]
fn pin_geometry_is_sanitized() {
    let temp = TempDir::new("pin-sanitize");
    let mut store = store_with_workspace(&temp);
    let sticky = store.create_sticky().expect("create sticky");

    store
        .pin_sticky(&sticky.id, f32::NAN, f32::INFINITY, 10.0, 10_000.0)
        .expect("pin with non-finite geometry");
    let pinned = store.pinned_stickies();
    assert_eq!(pinned[0].x, 0.0);
    assert_eq!(pinned[0].y, 0.0);
    assert_eq!(pinned[0].width, PINNED_STICKY_MIN_WIDTH);
    assert_eq!(pinned[0].height, PINNED_STICKY_MAX_HEIGHT);

    store
        .pin_sticky(&sticky.id, 5.0, 6.0, f32::NAN, f32::NEG_INFINITY)
        .expect("pin with non-finite size");
    let pinned = store.pinned_stickies();
    assert!((pinned[0].x - 5.0).abs() < f32::EPSILON);
    assert!((pinned[0].y - 6.0).abs() < f32::EPSILON);
    assert!(pinned[0].width.is_finite());
    assert!(pinned[0].height.is_finite());
    assert!((PINNED_STICKY_MIN_WIDTH..=PINNED_STICKY_MAX_WIDTH).contains(&pinned[0].width));
    assert!((PINNED_STICKY_MIN_HEIGHT..=PINNED_STICKY_MAX_HEIGHT).contains(&pinned[0].height));

    assert!(
        store
            .pin_sticky("missing-sticky", 0.0, 0.0, 240.0, 180.0)
            .is_err()
    );
}

#[test]
fn malformed_pinned_geometry_is_sanitized_on_load() {
    let temp = TempDir::new("pinned-sanitize-load");
    write_raw_workspace_manifest(
        &temp,
        r#"version = 1
id = "exe-testgame"
display_name = "Test Game"

[[stickies]]
id = "sticky-a"
text = "a"

[[pinned_stickies]]
sticky_id = "sticky-a"
x = 42.0
y = 43.0
width = 300.0
height = 250.0

[[pinned_stickies]]
sticky_id = "sticky-a"
x = "not a number"

[[pinned_stickies]]
sticky_id = "sticky-a"
x = nan
y = inf
width = -50.0
height = 100000.0
"#,
    );

    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    let workspace = store
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("open workspace with malformed pins");
    let pinned = workspace.pinned_stickies();
    assert_eq!(
        pinned.len(),
        1,
        "malformed entry is dropped, duplicates merged"
    );
    assert_eq!(pinned[0].sticky_id, "sticky-a");
    assert_eq!(pinned[0].x, 0.0);
    assert_eq!(pinned[0].y, 0.0);
    assert_eq!(pinned[0].width, PINNED_STICKY_MIN_WIDTH);
    assert_eq!(pinned[0].height, PINNED_STICKY_MAX_HEIGHT);
}

#[test]
fn stale_pins_are_dropped_on_load() {
    let temp = TempDir::new("pinned-stale");
    write_raw_workspace_manifest(
        &temp,
        r#"version = 1
id = "exe-testgame"
display_name = "Test Game"

[[stickies]]
id = "sticky-a"
text = "a"

[[pinned_stickies]]
sticky_id = "ghost"
x = 1.0
y = 2.0
width = 240.0
height = 180.0

[[pinned_stickies]]
sticky_id = "sticky-a"
x = 30.0
y = 40.0
width = 240.0
height = 180.0
"#,
    );

    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    let workspace = store
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("open workspace with stale pin");
    let pinned = workspace.pinned_stickies();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].sticky_id, "sticky-a");

    // The cleaned manifest is persisted on open, so the stale pin is gone.
    let manifest = fs::read_to_string(
        temp.path()
            .join("workspaces")
            .join("exe-testgame")
            .join("workspace.toml"),
    )
    .expect("read persisted manifest");
    assert!(
        !manifest.contains("ghost"),
        "stale pin should not be rewritten"
    );
    assert!(manifest.contains("sticky-a"));
}

#[test]
fn move_pinned_sticky_updates_position() {
    let temp = TempDir::new("pinned-move");
    let mut store = store_with_workspace(&temp);
    let sticky = store.create_sticky().expect("create sticky");
    store
        .pin_sticky(&sticky.id, 10.0, 20.0, 300.0, 200.0)
        .expect("pin sticky");

    store
        .move_pinned_sticky(&sticky.id, -150.0, 240.5)
        .expect("move pinned sticky");
    let pinned = store.pinned_stickies();
    assert!((pinned[0].x - -150.0).abs() < f32::EPSILON);
    assert!((pinned[0].y - 240.5).abs() < f32::EPSILON);
    assert!((pinned[0].width - 300.0).abs() < f32::EPSILON);
    assert!(
        store
            .move_pinned_sticky("missing-sticky", 0.0, 0.0)
            .is_err()
    );
    store
        .move_pinned_sticky(&sticky.id, f32::NAN, f32::NEG_INFINITY)
        .expect("move sanitizes non-finite coordinates");
    assert_eq!(store.pinned_stickies()[0].x, 0.0);
    assert_eq!(store.pinned_stickies()[0].y, 0.0);
    drop(store);

    let mut reopened = Store::bootstrap_at(temp.path()).expect("reopen store");
    reopened
        .open_workspace(WorkspaceDescriptor::new("exe-testgame", "Test Game"))
        .expect("reopen workspace");
    assert_eq!(reopened.pinned_stickies().len(), 1);
    assert_eq!(reopened.pinned_stickies()[0].x, 0.0);
    assert_eq!(reopened.pinned_stickies()[0].y, 0.0);
}

#[test]
fn pinned_api_requires_an_open_workspace() {
    let temp = TempDir::new("pinned-no-workspace");
    let mut store = Store::bootstrap_at(temp.path()).expect("bootstrap store");
    assert_eq!(store.pinned_stickies(), []);
    assert!(!store.is_sticky_pinned("anything"));
    assert!(
        store
            .pin_sticky("anything", 0.0, 0.0, 240.0, 180.0)
            .is_err()
    );
    assert!(store.unpin_sticky("anything").is_err());
    assert!(store.move_pinned_sticky("anything", 0.0, 0.0).is_err());
}
