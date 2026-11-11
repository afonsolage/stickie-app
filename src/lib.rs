//! Stickie: a lightweight in-game overlay for notes, stickies and screenshots.
//!
//! The crate is split into four layers:
//!
//! - [`store`]: app data, per-game workspaces and all user files. No Bevy.
//! - [`platform`]: the only OS-specific code (game probe, capture, global
//!   hotkey, shell integration).
//! - [`ui`]: the egui overlay shell and tabs.
//! - [`app`]: Bevy wiring and the overlay/capture state machine.
//!
//! The binary entry point lives in `main.rs` and only calls [`run`].

pub mod app;
pub mod platform;
pub mod store;
pub mod ui;

pub use app::run;
