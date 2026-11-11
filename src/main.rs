use bevy::prelude::*;
use bevy::window::{CompositeAlphaMode, MonitorSelection, WindowLevel, WindowMode, WindowPlugin};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Stickie".into(),
                mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
                transparent: true,
                decorations: false,
                window_level: WindowLevel::AlwaysOnTop,
                composite_alpha_mode: CompositeAlphaMode::Auto,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::NONE))
        .add_plugins(EguiPlugin::default())
        .add_systems(Startup, setup)
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn ui(mut contexts: EguiContexts) -> Result {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_black_alpha(100)))
        .show(contexts.ctx_mut()?, |ui| {
            ui.heading("Stickie");
        });
    Ok(())
}
