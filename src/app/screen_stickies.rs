//! Separate always-on-top windows for stickies pinned to the screen.
//!
//! Each pinned sticky owns a small Bevy window with its own egui context and
//! schedule. The windows live outside the main overlay window, so they stay on
//! screen while Stickie itself is hidden, but they are hidden while the main
//! overlay is open (it covers the whole monitor) and during capture.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bevy::camera::RenderTarget;
use bevy::ecs::schedule::{Schedule, ScheduleLabel};
use bevy::prelude::*;
use bevy::window::{
    CompositeAlphaMode, PresentMode, WindowLevel, WindowMode, WindowMoved, WindowPosition,
    WindowRef, WindowResolution,
};
use bevy_egui::{EguiContext, EguiSchedule, egui};

use super::{APP_NAME, AppCommand, OverlayState, StoreResource};
use crate::ui::screen_sticky::{self, ScreenStickyAction};

/// Maximum number of pinned screen sticky windows.
pub(super) const MAX_SCREEN_STICKIES: usize = 12;
/// Default logical/physical size of a screen sticky.
const DEFAULT_SCREEN_STICKY_WIDTH: f32 = 300.0;
const DEFAULT_SCREEN_STICKY_HEIGHT: f32 = 220.0;
/// Debounce before a moved window position is persisted.
const POSITION_SAVE_DELAY: Duration = Duration::from_millis(300);

/// A unique schedule label for one sticky window's egui pass.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct StickyWindowSchedule(u64);

impl ScheduleLabel for StickyWindowSchedule {
    fn dyn_clone(&self) -> Box<dyn ScheduleLabel> {
        Box::new(self.clone())
    }
}

/// One live Bevy window/camera pair for a pinned sticky.
#[derive(Clone, Copy)]
struct ScreenStickyInstance {
    window: Entity,
    camera: Entity,
}

/// Tracks the Bevy windows that represent pinned stickies.
#[derive(Resource, Default)]
pub(super) struct ScreenStickyState {
    instances: HashMap<String, ScreenStickyInstance>,
    next_schedule_id: u64,
}

/// Pending window positions from native dragging, persisted after a short
/// debounce so a drag does not rewrite the workspace manifest every frame.
#[derive(Resource, Default)]
struct ScreenStickyMoves {
    pending: HashMap<String, (f32, f32)>,
    changed_at: Option<Instant>,
}

/// Registers the screen-sticky lifecycle systems.
pub(super) struct ScreenStickyPlugin;

impl Plugin for ScreenStickyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenStickyState>()
            .init_resource::<ScreenStickyMoves>()
            .add_systems(
                Update,
                (
                    sync_screen_stickies.after(super::handle_commands),
                    update_screen_sticky_visibility.after(sync_screen_stickies),
                    collect_screen_sticky_moves,
                    flush_screen_sticky_moves.after(collect_screen_sticky_moves),
                ),
            );
    }
}

/// Geometry for a newly pinned sticky, cascaded near the top-right of the
/// primary display.
pub(super) fn next_pin_geometry(
    count: usize,
    (monitor_width, monitor_height): (u32, u32),
) -> (f32, f32, f32, f32) {
    let width = DEFAULT_SCREEN_STICKY_WIDTH.min((monitor_width as f32 - 24.0).max(180.0));
    let height = DEFAULT_SCREEN_STICKY_HEIGHT.min((monitor_height as f32 - 24.0).max(100.0));
    let cascade = (count % 6) as f32 * 28.0;
    let x = (monitor_width as f32 - width - 36.0 - cascade)
        .clamp(8.0, (monitor_width as f32 - width - 8.0).max(8.0));
    let y = (68.0 + cascade).clamp(8.0, (monitor_height as f32 - height - 8.0).max(8.0));
    (x, y, width, height)
}

fn screen_sticky_window(pin: &crate::store::PinnedSticky) -> Window {
    let width = pin.width.max(1.0) as u32;
    let height = pin.height.max(1.0) as u32;
    Window {
        title: format!("{APP_NAME} Sticky"),
        mode: WindowMode::Windowed,
        position: WindowPosition::At(IVec2::new(pin.x.round() as i32, pin.y.round() as i32)),
        resolution: WindowResolution::new(width, height),
        transparent: true,
        decorations: false,
        resizable: false,
        window_level: WindowLevel::AlwaysOnTop,
        skip_taskbar: true,
        visible: false,
        focused: false,
        composite_alpha_mode: if cfg!(target_os = "windows") {
            CompositeAlphaMode::PreMultiplied
        } else {
            CompositeAlphaMode::Auto
        },
        present_mode: PresentMode::AutoVsync,
        ..default()
    }
}

/// Reconciles the pinned list in the workspace with the live windows.
fn sync_screen_stickies(world: &mut World) {
    let Some(pinned) = world
        .get_resource::<StoreResource>()
        .map(|store| store.pinned_stickies().to_vec())
    else {
        return;
    };
    // A display change can leave a persisted card off-screen. Keep it inside
    // the primary monitor and persist the corrected position below.
    let mut geometry_updates = Vec::new();
    let mut pinned = pinned;
    if let Some((monitor_width, monitor_height)) = crate::platform::primary_monitor_size() {
        for pin in &mut pinned {
            let max_x = (monitor_width as f32 - pin.width).max(0.0);
            let max_y = (monitor_height as f32 - pin.height).max(0.0);
            let x = pin.x.clamp(0.0, max_x);
            let y = pin.y.clamp(0.0, max_y);
            if (x - pin.x).abs() > 0.5 || (y - pin.y).abs() > 0.5 {
                pin.x = x;
                pin.y = y;
                geometry_updates.push((pin.sticky_id.clone(), x, y));
            }
        }
    }
    let sticky_ids: std::collections::HashSet<String> = world
        .get_resource::<StoreResource>()
        .map(|store| {
            store
                .stickies()
                .iter()
                .map(|sticky| sticky.id.clone())
                .collect()
        })
        .unwrap_or_default();

    let mut state = {
        let mut resource = world.resource_mut::<ScreenStickyState>();
        std::mem::take(&mut *resource)
    };
    let desired: std::collections::HashSet<&str> =
        pinned.iter().map(|pin| pin.sticky_id.as_str()).collect();

    let stale: Vec<String> = state
        .instances
        .keys()
        .filter(|id| !desired.contains(id.as_str()) || !sticky_ids.contains(*id))
        .cloned()
        .collect();
    for id in stale {
        if let Some(instance) = state.instances.remove(&id) {
            if world.get_entity(instance.camera).is_ok() {
                world.despawn(instance.camera);
            }
            if world.get_entity(instance.window).is_ok() {
                world.despawn(instance.window);
            }
        }
    }

    for pin in pinned {
        if state.instances.contains_key(&pin.sticky_id) || !sticky_ids.contains(&pin.sticky_id) {
            continue;
        }
        let schedule_id = state.next_schedule_id;
        state.next_schedule_id = state.next_schedule_id.wrapping_add(1);
        let label = StickyWindowSchedule(schedule_id);
        let window_entity = world.spawn(screen_sticky_window(&pin)).id();
        let camera_entity = world
            .spawn((
                Camera2d,
                RenderTarget::Window(WindowRef::Entity(window_entity)),
                EguiSchedule::new(label.clone()),
            ))
            .id();

        let mut window_schedule = Schedule::new(label);
        window_schedule.add_systems(screen_sticky_draw_system(
            pin.sticky_id.clone(),
            window_entity,
            camera_entity,
        ));
        world.add_schedule(window_schedule);
        state.instances.insert(
            pin.sticky_id,
            ScreenStickyInstance {
                window: window_entity,
                camera: camera_entity,
            },
        );
    }
    world.insert_resource(state);
    if !geometry_updates.is_empty() {
        let mut store = world.resource_mut::<StoreResource>();
        for (sticky_id, x, y) in geometry_updates {
            if let Err(error) = store.move_pinned_sticky(&sticky_id, x, y) {
                log::warn!("could not clamp sticky window position: {error:#}");
            }
        }
    }
}

/// Hide pinned windows while the main full-screen overlay is open. The overlay
/// covers the monitor, so showing tiny windows above it would only obscure the
/// panel; when the overlay hides they reappear over the game.
fn update_screen_sticky_visibility(
    overlay: Res<OverlayState>,
    state: Res<ScreenStickyState>,
    mut windows: Query<&mut Window>,
) {
    let visible = !overlay.visible;
    for instance in state.instances.values() {
        if let Ok(mut window) = windows.get_mut(instance.window)
            && window.visible != visible
        {
            window.visible = visible;
        }
    }
}

/// Collect native window moves for pinned stickies.
fn collect_screen_sticky_moves(
    mut moved: MessageReader<WindowMoved>,
    state: Res<ScreenStickyState>,
    mut moves: ResMut<ScreenStickyMoves>,
) {
    for event in moved.read() {
        let Some(sticky_id) = state
            .instances
            .iter()
            .find_map(|(id, instance)| (instance.window == event.window).then_some(id))
        else {
            continue;
        };
        moves.pending.insert(
            sticky_id.clone(),
            (event.position.x as f32, event.position.y as f32),
        );
        moves.changed_at = Some(Instant::now());
    }
}

/// Persist dragged positions after the user stops moving the window.
fn flush_screen_sticky_moves(
    mut moves: ResMut<ScreenStickyMoves>,
    mut store: ResMut<StoreResource>,
) {
    if moves.pending.is_empty() {
        return;
    }
    let due = moves
        .changed_at
        .is_some_and(|changed| changed.elapsed() >= POSITION_SAVE_DELAY);
    if !due {
        return;
    }
    for (sticky_id, (x, y)) in moves.pending.drain() {
        if let Err(error) = store.move_pinned_sticky(&sticky_id, x, y) {
            log::warn!("could not persist sticky window position: {error:#}");
        }
    }
    moves.changed_at = None;
}

/// Build the per-window egui schedule system for one pinned sticky.
fn screen_sticky_draw_system(
    sticky_id: String,
    window_entity: Entity,
    camera_entity: Entity,
) -> impl FnMut(
    Query<&mut EguiContext>,
    Res<StoreResource>,
    Query<&mut Window>,
    MessageWriter<AppCommand>,
) + Send
+ Sync
+ 'static {
    move |mut contexts, store, mut windows, mut commands| {
        let Some(sticky) = store
            .stickies()
            .iter()
            .find(|sticky| sticky.id == sticky_id)
        else {
            return;
        };
        let Ok(mut context) = contexts.get_mut(camera_entity) else {
            return;
        };
        let ctx = context.get_mut().clone();
        let screen = ctx.viewport_rect();
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new(("screen-sticky", &sticky_id)),
            egui::UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(screen),
        );
        let response = screen_sticky::show(&mut ui, &sticky.text, sticky.color);
        if response.drag_requested
            && let Ok(mut window) = windows.get_mut(window_entity)
        {
            window.start_drag_move();
        }
        match response.action {
            Some(ScreenStickyAction::Edit) => {
                commands.write(AppCommand::OpenStickies(sticky_id.clone()));
            }
            Some(ScreenStickyAction::Unpin) => {
                commands.write(AppCommand::UnpinSticky(sticky_id.clone()));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_geometry_stays_inside_the_monitor() {
        for monitor in [(1920, 1080), (2560, 1080), (1440, 900), (1280, 720)] {
            for count in 0..MAX_SCREEN_STICKIES {
                let (x, y, width, height) = next_pin_geometry(count, monitor);
                assert!(width > 0.0 && height > 0.0);
                assert!(x >= 0.0 && y >= 0.0);
                assert!(
                    x + width <= monitor.0 as f32 + 0.01,
                    "count {count} monitor {monitor:?}: x={x} width={width}"
                );
                assert!(
                    y + height <= monitor.1 as f32 + 0.01,
                    "count {count} monitor {monitor:?}: y={y} height={height}"
                );
            }
        }
    }
}
