//! Application bootstrap and overlay controller.
//!
//! Bevy owns the event loop, the single transparent overlay window and the egui
//! render pass. The controller only does meaningful work when the overlay is
//! visible; hidden, the app sleeps in winit's low-power update mode until the
//! global hotkey wakes it.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::window::{
    CompositeAlphaMode, PresentMode, PrimaryWindow, WindowLevel, WindowMode, WindowPlugin,
    WindowPosition, WindowResolution,
};
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::platform::{self, GameContext};
use crate::store::{Store, WorkspaceDescriptor};
use crate::ui::{OverlayUi, UiAction, UiView};

/// The application name used in titles, paths and logs.
pub const APP_NAME: &str = "Stickie";

/// Environment variable that forces a visible window at startup (useful on
/// Linux and for screenshot runs).
pub const ENV_SHOW_ON_START: &str = "STICKIE_SHOW_ON_START";

/// Bevy resource wrapper around [`Store`].
///
/// The store is intentionally free of Bevy dependencies, so it cannot derive
/// `Resource` itself. This newtype keeps the dependency boundary clean while
/// still letting systems take `ResMut<StoreResource>` and use it as a store via
/// `Deref`/`DerefMut`.
#[derive(Resource)]
struct StoreResource(Store);

impl StoreResource {
    fn get(&self) -> &Store {
        &self.0
    }

    fn get_mut(&mut self) -> &mut Store {
        &mut self.0
    }
}

impl Deref for StoreResource {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for StoreResource {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Commands produced by platform input handlers and the egui layer.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    ToggleOverlay,
    HideOverlay,
    Quit,
    CaptureScreenshot,
    RebindHotkey(String),
    RevealDataDir,
    OpenPath(PathBuf),
}

/// A request to register a different global hotkey. Handled by the platform
/// layer on the event-loop thread.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
struct HotkeyRebindRequest(String);

/// Global overlay/window state.
#[derive(Resource, Default)]
struct OverlayState {
    visible: bool,
    game: Option<GameContext>,
}

/// A pending request to focus the overlay window after it becomes visible.
#[derive(Resource, Default)]
struct FocusRequest {
    active: bool,
    focus_sent: bool,
    started_at: Option<Instant>,
    /// Set a little after showing: the first focus attempt must happen after
    /// winit has mapped the previously hidden window.
    not_before: Option<Instant>,
}

/// The OS-level hotkey callback sets this flag and wakes the winit event loop.
#[derive(Resource, Clone)]
struct HotkeyPulse(Arc<AtomicBool>);

impl Default for HotkeyPulse {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

/// User-facing information about the active global hotkey.
#[derive(Resource, Clone, Debug)]
struct HotkeyInfo {
    active: String,
    warning: Option<String>,
}

/// Screenshot capture lifecycle. Capture runs on a worker thread so the render
/// loop and input handling never block.
#[derive(Resource, Default)]
struct CaptureState {
    phase: CapturePhase,
}

#[derive(Default)]
enum CapturePhase {
    #[default]
    Idle,
    HideThenCapture {
        started_at: Instant,
    },
    Waiting(Mutex<Receiver<anyhow::Result<image::RgbaImage>>>),
}

/// Internal plugin that registers all Stickie systems.
pub struct StickiePlugin;

impl Plugin for StickiePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AppCommand>()
            .add_message::<HotkeyRebindRequest>()
            .init_resource::<FocusRequest>()
            .init_resource::<CaptureState>()
            .add_systems(Startup, (setup_camera, startup_workspace))
            .add_systems(
                Update,
                (
                    poll_global_hotkey,
                    dev_hotkeys,
                    handle_commands,
                    drive_capture,
                    drive_focus,
                )
                    .chain(),
            )
            .add_systems(EguiPrimaryContextPass, draw_overlay);

        #[cfg(target_os = "windows")]
        {
            app.add_systems(Startup, install_global_hotkey)
                .add_systems(Update, apply_hotkey_rebind);
        }
        #[cfg(not(target_os = "windows"))]
        {
            app.add_systems(Update, apply_hotkey_rebind_stub);
        }
    }
}

/// Start Stickie. Bootstrapping the store happens before Bevy starts so a
/// broken data directory fails with a useful error instead of a panic inside a
/// plugin.
pub fn run() -> anyhow::Result<()> {
    // Must happen before the render plugin picks a wgpu backend/presenter.
    platform::configure_overlay_graphics();
    let store = Store::bootstrap()?;
    let start_visible = should_start_visible(&store);

    let configured = store.config().toggle_hotkey.clone();
    let mut hotkey_info = HotkeyInfo {
        active: platform::hotkey::hotkey_label(&configured),
        warning: None,
    };
    if !platform::hotkey::supported() {
        hotkey_info.active = "Ctrl+Shift+Tab (dev)".to_string();
        // The Windows-only limitation is shown in Settings/platform label; do
        // not flag it as a broken hotkey in the dev UI.
        hotkey_info.warning = None;
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(overlay_window(start_visible)),
        ..default()
    }))
    .add_plugins(EguiPlugin::default())
    .insert_resource(ClearColor(Color::NONE))
    .insert_resource(StoreResource(store))
    .insert_resource(OverlayUi::new())
    .insert_resource(OverlayState {
        visible: start_visible,
        game: None,
    })
    .insert_resource(idle_winit_settings())
    .insert_resource(HotkeyPulse::default())
    .insert_resource(hotkey_info)
    .add_plugins(StickiePlugin);

    app.run();
    Ok(())
}

/// Winit power profile used whenever the overlay is hidden or idle. The
/// unfocused wait is effectively forever: the only things that wake the loop
/// are real OS events (the global hotkey sends a `WakeUp`) or a requested
/// redraw. That is what keeps Stickie at zero-ish idle CPU during gameplay.
fn idle_winit_settings() -> WinitSettings {
    WinitSettings {
        focused_mode: UpdateMode::reactive(Duration::from_secs(1)),
        unfocused_mode: UpdateMode::reactive_low_power(Duration::from_secs(3600)),
    }
}

fn should_start_visible(store: &Store) -> bool {
    if std::env::var_os(ENV_SHOW_ON_START).is_some() {
        return true;
    }
    // Linux/macOS builds are development builds until the platform layer grows
    // a native hotkey backend, so show the UI by default there.
    if !cfg!(target_os = "windows") {
        return true;
    }
    !store.config().start_hidden
}

fn overlay_window(start_visible: bool) -> Window {
    // A borderless *windowed* surface covering the primary monitor is used on
    // Windows instead of `WindowMode::BorderlessFullscreen`. This keeps the
    // window on the normal DWM/DirectComposition presentation path; the
    // fullscreen path is what previously produced a black, opaque background.
    #[cfg(target_os = "windows")]
    let (mode, position, resolution, composite_alpha_mode) = {
        let (width, height) = platform::primary_monitor_size().unwrap_or((1920, 1080));
        (
            WindowMode::Windowed,
            WindowPosition::At(IVec2::ZERO),
            WindowResolution::new(width, height),
            CompositeAlphaMode::PreMultiplied,
        )
    };
    #[cfg(not(target_os = "windows"))]
    let (mode, position, resolution, composite_alpha_mode) = (
        WindowMode::Windowed,
        WindowPosition::Automatic,
        WindowResolution::new(1440, 900),
        CompositeAlphaMode::Auto,
    );

    Window {
        title: format!("{APP_NAME} Overlay"),
        mode,
        position,
        resolution,
        transparent: true,
        decorations: false,
        resizable: false,
        window_level: WindowLevel::AlwaysOnTop,
        skip_taskbar: true,
        visible: start_visible,
        focused: start_visible,
        composite_alpha_mode,
        present_mode: PresentMode::AutoVsync,
        ..default()
    }
}

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// When the app starts already visible (config, dev mode or registration
/// failure), open the matching workspace immediately.
fn startup_workspace(
    mut store: ResMut<StoreResource>,
    mut ui: ResMut<OverlayUi>,
    mut overlay: ResMut<OverlayState>,
) {
    if !overlay.visible {
        return;
    }
    let game = platform::active_game();
    match open_workspace(store.get_mut(), game.as_ref()) {
        Ok(()) => {
            overlay.game = game;
            ui.on_workspace_changed(store.get());
        }
        Err(error) => {
            log::error!("failed to open startup workspace: {error:#}");
            ui.push_error(format!("Could not open workspace: {error}"));
        }
    }
}

fn open_workspace(store: &mut Store, game: Option<&GameContext>) -> anyhow::Result<()> {
    let descriptor = match game {
        Some(game) => WorkspaceDescriptor {
            workspace_id: &game.workspace_id,
            display_name: &game.display_name,
            executable: game.executable.as_deref(),
            window_title: game.window_title.as_deref(),
        },
        None => WorkspaceDescriptor {
            workspace_id: "desktop",
            display_name: "Desktop",
            executable: None,
            window_title: None,
        },
    };
    store.open_workspace(descriptor)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_global_hotkey(world: &mut World) {
    use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
    use std::ops::Deref;

    let configured = world
        .resource::<StoreResource>()
        .config()
        .toggle_hotkey
        .clone();
    let pulse = world.resource::<HotkeyPulse>().0.clone();
    let proxy = world.resource::<EventLoopProxyWrapper>().deref().clone();

    let on_press = {
        let pulse = pulse.clone();
        let proxy = proxy.clone();
        move || {
            pulse.store(true, Ordering::Release);
            let _ = proxy.send_event(WinitUserEvent::WakeUp);
        }
    };

    match platform::hotkey::register_with_fallback(&configured, on_press) {
        Ok((handle, active, warning)) => {
            let mut info = world.resource_mut::<HotkeyInfo>();
            info.active = active;
            info.warning = warning;
            world.insert_non_send(GlobalHotkeyState {
                handle: Some(handle),
            });
        }
        Err(error) => {
            log::error!("no global hotkey could be registered: {error:#}");
            {
                let mut info = world.resource_mut::<HotkeyInfo>();
                info.warning = Some(format!(
                    "No global hotkey is registered ({error}). Fix it in Settings, or use the \
                     overlay buttons while it is open."
                ));
            }
            world.insert_non_send(GlobalHotkeyState { handle: None });
            // Do not leave the process invisible and unreachable: open the
            // overlay so the user can fix the binding.
            world.resource_mut::<OverlayState>().visible = true;
            if let Some(mut window) = world.query::<&mut Window>().iter_mut(world).next() {
                window.visible = true;
                window.focused = true;
            }
            let game = platform::active_game();
            let workspace_opened = {
                let mut store = world.resource_mut::<StoreResource>();
                match open_workspace(store.get_mut(), game.as_ref()) {
                    Ok(()) => true,
                    Err(error) => {
                        log::error!("failed to open workspace after hotkey failure: {error:#}");
                        false
                    }
                }
            };
            if workspace_opened {
                world.resource_scope(|world, mut ui: Mut<OverlayUi>| {
                    let store = world.resource::<StoreResource>();
                    ui.on_workspace_changed(store.get());
                });
            }
            world.resource_mut::<OverlayState>().game = game;
        }
    }
}

/// Windows-only non-send hotkey handle. Bevy non-send resources are dropped on
/// the main thread, which is where `GlobalHotKeyManager` requires its lifecycle
/// to live.
#[cfg(target_os = "windows")]
struct GlobalHotkeyState {
    handle: Option<platform::hotkey::HotkeyHandle>,
}

fn poll_global_hotkey(pulse: Res<HotkeyPulse>, mut commands: MessageWriter<AppCommand>) {
    if pulse.0.swap(false, Ordering::Acquire) {
        commands.write(AppCommand::ToggleOverlay);
    }
}

/// Local development shortcut. The real product uses the OS-level hotkey; this
/// keeps Linux/Windows-dev builds usable even when the global binding is
/// unavailable.
fn dev_hotkeys(keys: Res<ButtonInput<KeyCode>>, mut commands: MessageWriter<AppCommand>) {
    if cfg!(target_os = "windows") {
        return;
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if ctrl && shift && keys.just_pressed(KeyCode::Tab) {
        commands.write(AppCommand::ToggleOverlay);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_commands(
    mut commands: MessageReader<AppCommand>,
    mut hotkey_requests: MessageWriter<HotkeyRebindRequest>,
    mut store: ResMut<StoreResource>,
    mut ui: ResMut<OverlayUi>,
    mut overlay: ResMut<OverlayState>,
    mut capture: ResMut<CaptureState>,
    mut focus: ResMut<FocusRequest>,
    mut hotkey_info: ResMut<HotkeyInfo>,
    mut winit_settings: ResMut<WinitSettings>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    for command in commands.read() {
        match command {
            AppCommand::ToggleOverlay => {
                if !matches!(capture.phase, CapturePhase::Idle) || ui.is_capturing() {
                    continue;
                }
                if overlay.visible {
                    hide_overlay(
                        &mut ui,
                        store.get_mut(),
                        &mut overlay,
                        &mut focus,
                        &mut winit_settings,
                        window.as_mut(),
                    );
                } else {
                    show_overlay(
                        &mut ui,
                        store.get_mut(),
                        &mut overlay,
                        &mut focus,
                        &mut winit_settings,
                        window.as_mut(),
                    );
                }
            }
            AppCommand::HideOverlay => {
                if !matches!(capture.phase, CapturePhase::Idle) || ui.is_capturing() {
                    continue;
                }
                if overlay.visible {
                    hide_overlay(
                        &mut ui,
                        store.get_mut(),
                        &mut overlay,
                        &mut focus,
                        &mut winit_settings,
                        window.as_mut(),
                    );
                }
            }
            AppCommand::Quit => {
                ui.flush(store.get_mut());
                if let Some(game) = &overlay.game {
                    platform::restore_game_focus(game);
                }
                exit.write(AppExit::Success);
            }
            AppCommand::CaptureScreenshot => {
                if matches!(capture.phase, CapturePhase::Idle)
                    && !ui.is_capturing()
                    && overlay.visible
                {
                    ui.flush(store.get_mut());
                    overlay.visible = false;
                    focus.active = false;
                    window.visible = false;
                    winit_settings.unfocused_mode =
                        UpdateMode::reactive_low_power(Duration::from_millis(16));
                    capture.phase = CapturePhase::HideThenCapture {
                        started_at: Instant::now(),
                    };
                }
            }
            AppCommand::RebindHotkey(spec) => {
                let spec = spec.trim().to_string();
                if spec.is_empty() {
                    ui.push_error("Hotkey cannot be empty.");
                    continue;
                }
                hotkey_info.active = platform::hotkey::hotkey_label(&spec);
                ui.push_info(format!("Registering {}…", hotkey_info.active));
                hotkey_requests.write(HotkeyRebindRequest(spec));
            }
            AppCommand::RevealDataDir => {
                if let Err(error) = platform::open_in_file_manager(store.paths().root()) {
                    ui.push_error(format!("Could not open data folder: {error}"));
                }
            }
            AppCommand::OpenPath(path) => {
                if let Err(error) = platform::open_in_file_manager(path) {
                    ui.push_error(format!("Could not open {}: {error}", path.display()));
                }
            }
        }
    }
}

fn show_overlay(
    ui: &mut OverlayUi,
    store: &mut Store,
    overlay: &mut OverlayState,
    focus: &mut FocusRequest,
    winit_settings: &mut WinitSettings,
    window: &mut Window,
) {
    let game = platform::active_game();
    if let Err(error) = open_workspace(store, game.as_ref()) {
        log::error!("failed to open workspace: {error:#}");
        ui.push_error(format!("Could not open workspace: {error}"));
    } else {
        ui.on_workspace_changed(store);
    }
    overlay.game = game;
    overlay.visible = true;
    focus.active = true;
    focus.focus_sent = false;
    focus.started_at = Some(Instant::now());
    focus.not_before = Some(Instant::now() + Duration::from_millis(50));
    window.visible = true;
    // Keep updates flowing while the window is mapped and asks for focus.
    winit_settings.unfocused_mode = UpdateMode::reactive_low_power(Duration::from_millis(16));
}

fn hide_overlay(
    ui: &mut OverlayUi,
    store: &mut Store,
    overlay: &mut OverlayState,
    focus: &mut FocusRequest,
    winit_settings: &mut WinitSettings,
    window: &mut Window,
) {
    ui.flush(store);
    overlay.visible = false;
    focus.active = false;
    focus.focus_sent = false;
    focus.started_at = None;
    focus.not_before = None;
    window.visible = false;
    // Track that the next show needs a fresh focus request. Winit ignores
    // manual unfocus for the OS, but this syncs Bevy's cached focus state so a
    // later `focused = true` triggers winit's focus path again.
    window.focused = false;
    *winit_settings = idle_winit_settings();
    if let Some(game) = &overlay.game {
        platform::restore_game_focus(game);
    }
}

fn drive_focus(
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut focus: ResMut<FocusRequest>,
    overlay: Res<OverlayState>,
    mut winit_settings: ResMut<WinitSettings>,
) {
    if !focus.active || !overlay.visible {
        return;
    }
    if !window.visible {
        return;
    }
    if focus
        .not_before
        .is_some_and(|not_before| Instant::now() < not_before)
    {
        return;
    }
    if !focus.focus_sent {
        window.focused = true;
        focus.focus_sent = true;
    }
    let timed_out = focus
        .started_at
        .is_none_or(|started| started.elapsed() > Duration::from_millis(500));
    if window.focused || timed_out {
        focus.active = false;
        focus.started_at = None;
        focus.not_before = None;
        *winit_settings = idle_winit_settings();
    }
}

fn drive_capture(
    mut capture: ResMut<CaptureState>,
    mut ui: ResMut<OverlayUi>,
    mut overlay: ResMut<OverlayState>,
    mut focus: ResMut<FocusRequest>,
    mut winit_settings: ResMut<WinitSettings>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    // Take the phase out so `capture` isn't borrowed immutably while the
    // capture state is updated below. Every arm either installs a new phase or
    // leaves the state Idle.
    let phase = std::mem::take(&mut capture.phase);
    match phase {
        CapturePhase::Idle => {}
        CapturePhase::HideThenCapture { started_at } => {
            if started_at.elapsed() < Duration::from_millis(180) {
                capture.phase = CapturePhase::HideThenCapture { started_at };
                return;
            }
            let (sender, receiver) = mpsc::channel();
            match std::thread::Builder::new()
                .name("stickie-capture".to_string())
                .spawn(move || {
                    // Capture the whole primary display; the capture overlay
                    // lets the user select the region to keep.
                    let result = platform::capture_screen();
                    let _ = sender.send(result);
                }) {
                Ok(_) => capture.phase = CapturePhase::Waiting(Mutex::new(receiver)),
                Err(error) => {
                    ui.push_error(format!("Could not start screenshot thread: {error}"));
                    finish_capture(
                        &mut capture,
                        &mut overlay,
                        &mut focus,
                        &mut winit_settings,
                        &mut window,
                    );
                }
            }
        }
        CapturePhase::Waiting(receiver) => {
            let Ok(guard) = receiver.lock() else {
                log::error!("screenshot receiver lock poisoned");
                finish_capture(
                    &mut capture,
                    &mut overlay,
                    &mut focus,
                    &mut winit_settings,
                    &mut window,
                );
                return;
            };
            let result = match guard.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => {
                    drop(guard);
                    capture.phase = CapturePhase::Waiting(receiver);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    ui.push_error("Screenshot worker stopped unexpectedly.");
                    None
                }
            };
            drop(guard);
            if let Some(result) = result {
                match result {
                    Ok(image) => {
                        ui.begin_capture(image);
                    }
                    Err(error) => {
                        ui.push_error(format!("Screenshot failed: {error}"));
                    }
                }
            }
            finish_capture(
                &mut capture,
                &mut overlay,
                &mut focus,
                &mut winit_settings,
                &mut window,
            );
        }
    }
}

fn finish_capture(
    capture: &mut CaptureState,
    overlay: &mut OverlayState,
    focus: &mut FocusRequest,
    winit_settings: &mut WinitSettings,
    window: &mut Window,
) {
    capture.phase = CapturePhase::Idle;
    overlay.visible = true;
    focus.active = true;
    focus.focus_sent = false;
    focus.started_at = Some(Instant::now());
    focus.not_before = Some(Instant::now() + Duration::from_millis(50));
    window.visible = true;
    // Keep updates flowing for the delayed focus attempt; `drive_focus` puts
    // the app back into the idle profile once focus is settled.
    winit_settings.unfocused_mode = UpdateMode::reactive_low_power(Duration::from_millis(16));
}

fn draw_overlay(
    mut contexts: EguiContexts,
    mut ui: ResMut<OverlayUi>,
    mut store: ResMut<StoreResource>,
    overlay: Res<OverlayState>,
    hotkey: Res<HotkeyInfo>,
    mut commands: MessageWriter<AppCommand>,
) -> Result {
    if !overlay.visible {
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    let viewport_rect = ctx.viewport_rect();
    let mut viewport = egui::Ui::new(
        ctx.clone(),
        "stickie-viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(viewport_rect),
    );

    let view = UiView {
        workspace_name: overlay
            .game
            .as_ref()
            .map_or_else(|| "Desktop".to_string(), |game| game.display_name.clone()),
        game_title: overlay
            .game
            .as_ref()
            .and_then(|game| game.window_title.clone()),
        executable: overlay
            .game
            .as_ref()
            .and_then(|game| game.executable.clone()),
        hotkey_active: hotkey.active.clone(),
        hotkey_warning: hotkey.warning.clone(),
        overlay_supported: platform::overlay_supported(),
        capture_supported: cfg!(target_os = "windows"),
        scrim_opacity: store.config().scrim_opacity,
        platform: platform::platform_name().to_string(),
        data_root: store.paths().root().to_path_buf(),
    };

    ui.show(&mut viewport, store.get_mut(), &view);
    ui.autosave(store.get_mut());

    for action in ui.take_actions() {
        match action {
            UiAction::Hide => {
                commands.write(AppCommand::HideOverlay);
            }
            UiAction::Quit => {
                commands.write(AppCommand::Quit);
            }
            UiAction::CaptureScreenshot => {
                commands.write(AppCommand::CaptureScreenshot);
            }
            UiAction::RebindHotkey(spec) => {
                commands.write(AppCommand::RebindHotkey(spec));
            }
            UiAction::RevealDataDir => {
                commands.write(AppCommand::RevealDataDir);
            }
            UiAction::OpenPath(path) => {
                commands.write(AppCommand::OpenPath(path));
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_hotkey_rebind(
    mut requests: MessageReader<HotkeyRebindRequest>,
    mut hotkey_state: NonSendMut<GlobalHotkeyState>,
    mut info: ResMut<HotkeyInfo>,
    mut store: ResMut<StoreResource>,
    mut ui: ResMut<OverlayUi>,
) {
    for request in requests.read() {
        let Some(handle) = hotkey_state.handle.as_mut() else {
            info.warning = Some("No global hotkey backend is available.".to_string());
            ui.push_error("No global hotkey backend is available.");
            continue;
        };
        match handle.rebind(&request.0) {
            Ok(()) => {
                let active = handle.spec().to_string();
                info.active.clone_from(&active);
                info.warning = None;
                let mut config = store.config().clone();
                config.toggle_hotkey = active.clone();
                if let Err(error) = store.save_config(config) {
                    log::error!("could not persist hotkey: {error:#}");
                }
                ui.push_success(format!("Hotkey set to {active}."));
            }
            Err(error) => {
                let message = format!("Could not register hotkey: {error}");
                // The previous binding is still active (the platform layer
                // rolls back); keep the status bar honest about that.
                info.active = handle.spec().to_string();
                info.warning = Some(message.clone());
                ui.push_error(message);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_hotkey_rebind_stub(
    mut requests: MessageReader<HotkeyRebindRequest>,
    mut store: ResMut<StoreResource>,
    mut ui: ResMut<OverlayUi>,
    mut info: ResMut<HotkeyInfo>,
) {
    for request in requests.read() {
        let label = platform::hotkey::hotkey_label(&request.0);
        let mut config = store.config().clone();
        config.toggle_hotkey = request.0.clone();
        if let Err(error) = store.save_config(config) {
            ui.push_error(format!("Could not save hotkey: {error}"));
            continue;
        }
        info.active = format!("{label} (dev shortcut: Ctrl+Shift+Tab)");
        ui.push_info("Hotkey saved. Global registration is Windows-only in this build.");
    }
}
