//! Global hotkey backend.
//!
//! Stickie uses a real OS-level hotkey (`RegisterHotKey` through the
//! `global-hotkey` crate) so the overlay can be toggled while a game owns the
//! foreground and no input hook is installed.
//!
//! # Threading contract
//!
//! On Windows the [`GlobalHotKeyManager`] creates a message-only window and
//! must be created on the thread that owns the Win32 message loop. With Bevy
//! that is the main/event-loop thread, so [`register`] has to be called from a
//! `Startup` system and the returned [`HotkeyHandle`] stored as a **non-send**
//! resource. The press callback is invoked from that same message loop.
//!
//! # Specs
//!
//! A hotkey spec is a `+`-separated chord such as `"Win+Shift+Tab"`. The
//! configured spelling is normalised with [`normalize_hotkey_spec`] before it
//! is handed to `global-hotkey` (which does not understand `Win`), and
//! [`hotkey_label`] turns it back into a human readable form for the UI.
//!
//! Non-Windows builds expose the same functions as stubs that return a clear
//! "unsupported" error, so the app can be developed on Linux without `cfg`
//! gymnastics at every call site.

/// Hotkey used unless the user configures something else.
pub const DEFAULT_SPEC: &str = "Win+Shift+Tab";

/// Safe fallback when the configured hotkey cannot be registered, e.g. because
/// another application already owns the chord.
pub const FALLBACK_SPEC: &str = "Ctrl+Shift+Tab";

/// Returns `true` when this build registers real global hotkeys.
pub const fn supported() -> bool {
    cfg!(windows)
}

/// Normalises a user-facing hotkey spec into the token spelling understood by
/// `global-hotkey` and `keyboard-types`.
///
/// - `Win`/`Windows`/`Super`/`Meta`/`OS` become `super` (`global-hotkey` maps
///   that token to the Windows key; it does not accept `Win`).
/// - Common friendly aliases (`Esc`, `PgUp`, `Del`, `Ins`, ...) become the
///   names used by `keyboard_types::Code`.
/// - Modifiers are `ctrl`, `alt`, `shift` and `super`; tokens are trimmed,
///   lower-cased and empty tokens are dropped.
///
/// The function never fails: an unparsable spec produces an output that the
/// platform parser rejects, which surfaces as a normal registration error.
///
/// ```
/// # use stickie::platform::hotkey::normalize_hotkey_spec;
/// assert_eq!(normalize_hotkey_spec("Win+Shift+Tab"), "super+shift+tab");
/// assert_eq!(normalize_hotkey_spec("win + shift + tab"), "super+shift+tab");
/// assert_eq!(normalize_hotkey_spec("Ctrl+Esc"), "ctrl+escape");
/// ```
pub fn normalize_hotkey_spec(spec: &str) -> String {
    spec.split('+')
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                None
            } else {
                Some(normalize_hotkey_token(token))
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Formats a hotkey spec for display, e.g. `"super+shift+tab"` becomes
/// `"Win+Shift+Tab"`.
///
/// Unknown tokens are capitalised rather than rejected so a user typing a
/// custom spec still sees something reasonable in the UI.
///
/// ```
/// # use stickie::platform::hotkey::hotkey_label;
/// assert_eq!(hotkey_label("super+shift+tab"), "Win+Shift+Tab");
/// assert_eq!(hotkey_label("ctrl+shift+tab"), "Ctrl+Shift+Tab");
/// assert_eq!(hotkey_label("super+keya"), "Win+A");
/// ```
pub fn hotkey_label(spec: &str) -> String {
    spec.split('+')
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                None
            } else {
                Some(hotkey_label_token(token))
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Normalises one (already trimmed) hotkey token.
fn normalize_hotkey_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    match lower.as_str() {
        // `global-hotkey` does not understand "Win"; it parses SUPER/META as
        // the Windows key. Map every spelling of that modifier to `super`.
        "win" | "windows" | "super" | "meta" | "os" | "cmd" | "command" => "super".to_string(),
        "ctrl" | "control" => "ctrl".to_string(),
        "alt" | "option" | "opt" => "alt".to_string(),
        "shift" => "shift".to_string(),
        // Friendly aliases for `keyboard_types::Code` names.
        "esc" | "escape" => "escape".to_string(),
        "return" | "enter" => "enter".to_string(),
        "space" | "spacebar" => "space".to_string(),
        "tab" => "tab".to_string(),
        "backspace" | "bksp" => "backspace".to_string(),
        "del" | "delete" => "delete".to_string(),
        "ins" | "insert" => "insert".to_string(),
        "pgup" | "pageup" => "pageup".to_string(),
        "pgdn" | "pgdown" | "pagedown" => "pagedown".to_string(),
        "caps" | "capslock" => "capslock".to_string(),
        "prtsc" | "prtscr" | "printscreen" => "printscreen".to_string(),
        "scroll" | "scrolllock" => "scrolllock".to_string(),
        "up" | "arrowup" => "arrowup".to_string(),
        "down" | "arrowdown" => "arrowdown".to_string(),
        "left" | "arrowleft" => "arrowleft".to_string(),
        "right" | "arrowright" => "arrowright".to_string(),
        _ => lower,
    }
}

/// Formats one (already trimmed) hotkey token for display.
fn hotkey_label_token(token: &str) -> String {
    let normalized = token.to_ascii_lowercase();
    let fixed = match normalized.as_str() {
        "super" | "meta" | "win" | "windows" | "os" | "cmd" | "command" => Some("Win"),
        "ctrl" | "control" => Some("Ctrl"),
        "alt" | "option" | "opt" => Some("Alt"),
        "shift" => Some("Shift"),
        "escape" | "esc" => Some("Esc"),
        "tab" => Some("Tab"),
        "space" | "spacebar" => Some("Space"),
        "enter" | "return" => Some("Enter"),
        "backspace" | "bksp" => Some("Backspace"),
        "delete" | "del" => Some("Del"),
        "insert" | "ins" => Some("Ins"),
        "pageup" | "pgup" => Some("PgUp"),
        "pagedown" | "pgdn" | "pgdown" => Some("PgDn"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "capslock" | "caps" => Some("CapsLock"),
        "numlock" => Some("NumLock"),
        "scrolllock" | "scroll" => Some("ScrollLock"),
        "printscreen" | "prtsc" | "prtscr" => Some("PrintScreen"),
        "pause" | "break" => Some("Pause"),
        "arrowup" | "up" => Some("Up"),
        "arrowdown" | "down" => Some("Down"),
        "arrowleft" | "left" => Some("Left"),
        "arrowright" | "right" => Some("Right"),
        _ => None,
    };
    if let Some(label) = fixed {
        return label.to_string();
    }
    if let Some(letter) = normalized.strip_prefix("key")
        && letter.len() == 1
    {
        return letter.to_ascii_uppercase();
    }
    if let Some(digit) = normalized.strip_prefix("digit")
        && !digit.is_empty()
    {
        return digit.to_string();
    }
    if let Some(rest) = normalized.strip_prefix('f')
        && !rest.is_empty()
        && rest.chars().all(|character| character.is_ascii_digit())
    {
        return format!("F{rest}");
    }
    capitalize_ascii(&normalized)
}

/// Uppercases the first character of an ASCII-ish token, leaving the rest
/// alone.
fn capitalize_ascii(token: &str) -> String {
    let mut characters = token.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => {
            let mut capitalized = String::with_capacity(token.len());
            capitalized.extend(first.to_uppercase());
            capitalized.extend(characters);
            capitalized
        }
    }
}

#[cfg(windows)]
mod imp {
    //! Real Windows global hotkey registration.
    //!
    //! The `global-hotkey` crate owns a message-only window; its event handler
    //! is process-wide and can be installed only once. Stickie therefore keeps
    //! one process-wide press flag and one press callback in statics, while the
    //! manager/hotkey pair that must survive across frames lives in
    //! [`HotkeyHandle`].

    #![allow(unsafe_code)]

    use std::fmt;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, OnceLock};

    use anyhow::{Context, Result, anyhow};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use super::{hotkey_label, normalize_hotkey_spec};

    /// Set by the event handler and consumed by [`HotkeyHandle::pressed`]:
    /// one physical press is delivered exactly once.
    static PRESSED: AtomicBool = AtomicBool::new(false);

    /// Id of the hotkey currently registered with the process-wide manager.
    /// The handler ignores events for any other id (e.g. after a rebind).
    static CURRENT_HOTKEY_ID: AtomicU32 = AtomicU32::new(0);

    /// The caller-supplied wake callback. Installed once because
    /// `GlobalHotKeyEvent::set_event_handler` is process-wide.
    static ON_PRESS: OnceLock<Arc<dyn Fn() + Send + Sync + 'static>> = OnceLock::new();

    /// A registered global hotkey.
    ///
    /// Only one handle is expected per process. It is **not** `Send`: the
    /// underlying manager owns a Win32 window and must be dropped on the thread
    /// that owns the message loop, so store it as a Bevy non-send resource.
    pub struct HotkeyHandle {
        manager: GlobalHotKeyManager,
        hotkey: HotKey,
        spec: String,
    }

    impl fmt::Debug for HotkeyHandle {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("HotkeyHandle")
                .field("spec", &self.spec)
                .finish_non_exhaustive()
        }
    }

    impl HotkeyHandle {
        /// Consumes the press flag, returning `true` exactly once per press.
        pub fn pressed(&self) -> bool {
            PRESSED.swap(false, Ordering::AcqRel)
        }

        /// The human readable spec that is currently registered, e.g.
        /// `"Win+Shift+Tab"`.
        pub fn spec(&self) -> &str {
            &self.spec
        }

        /// Replaces the registered hotkey.
        ///
        /// The old binding is unregistered first. If the new spec cannot be
        /// registered the old binding is restored so the overlay keeps a
        /// working toggle, and the error is returned.
        pub fn rebind(&mut self, spec: &str) -> Result<()> {
            let label = hotkey_label(spec);
            let normalized = normalize_hotkey_spec(spec);
            let new_hotkey = parse_hotkey(&normalized, spec)?;

            if new_hotkey == self.hotkey {
                // Same chord, possibly a different spelling: keep the OS
                // registration and just refresh the label.
                self.spec = label;
                return Ok(());
            }

            if let Err(error) = self.manager.unregister(self.hotkey) {
                log::warn!(
                    "could not unregister hotkey {:?} while rebinding: {error}",
                    self.spec
                );
            }

            match self.manager.register(new_hotkey) {
                Ok(()) => {
                    self.hotkey = new_hotkey;
                    self.spec = label;
                    CURRENT_HOTKEY_ID.store(new_hotkey.id(), Ordering::Release);
                    PRESSED.store(false, Ordering::Release);
                    Ok(())
                }
                Err(error) => {
                    // Keep the previous binding alive when possible.
                    if let Err(restore_error) = self.manager.register(self.hotkey) {
                        log::error!(
                            "failed to restore hotkey {:?} after a failed rebind: {restore_error}",
                            self.spec
                        );
                    }
                    Err(anyhow!(
                        "failed to register hotkey {label:?} ({normalized:?}): {error}"
                    ))
                }
            }
        }
    }

    impl Drop for HotkeyHandle {
        fn drop(&mut self) {
            let _ = self.manager.unregister(self.hotkey);
            CURRENT_HOTKEY_ID.store(0, Ordering::Release);
            PRESSED.store(false, Ordering::Release);
        }
    }

    /// Registers `spec` and installs `on_press` as the process-wide press
    /// callback.
    ///
    /// # Threading
    ///
    /// Must be called on the thread that owns the Win32 message loop (the Bevy
    /// main thread, e.g. from `Startup`).
    ///
    /// # Errors
    ///
    /// Returns an error when the spec cannot be parsed, the manager cannot be
    /// created or the OS refuses the registration (e.g. another application
    /// already owns the chord).
    pub fn register(
        spec: &str,
        on_press: impl Fn() + Send + Sync + 'static,
    ) -> Result<HotkeyHandle> {
        let on_press: Arc<dyn Fn() + Send + Sync + 'static> = Arc::new(on_press);
        try_register(spec, &on_press)
    }

    /// Registers the configured spec, falling back to [`super::FALLBACK_SPEC`]
    /// when the configured chord cannot be registered.
    ///
    /// Returns the handle, the human readable spec that actually registered and
    /// an optional warning meant for the UI (set when the fallback was used).
    pub fn register_with_fallback(
        spec: &str,
        on_press: impl Fn() + Send + Sync + 'static,
    ) -> Result<(HotkeyHandle, String, Option<String>)> {
        let on_press: Arc<dyn Fn() + Send + Sync + 'static> = Arc::new(on_press);
        match try_register(spec, &on_press) {
            Ok(handle) => {
                let active = handle.spec().to_string();
                Ok((handle, active, None))
            }
            Err(primary_error) => {
                let fallback = super::FALLBACK_SPEC;
                log::warn!(
                    "could not register configured hotkey {spec:?}: {primary_error:#}; \
                     falling back to {fallback:?}"
                );
                match try_register(fallback, &on_press) {
                    Ok(handle) => {
                        let active = handle.spec().to_string();
                        let warning = format!(
                            "Could not register {spec}: {primary_error}. \
                             Falling back to {fallback}."
                        );
                        Ok((handle, active, Some(warning)))
                    }
                    Err(fallback_error) => Err(anyhow!(
                        "could not register any global hotkey: configured {spec:?}: \
                         {primary_error:#}; fallback {fallback:?}: {fallback_error:#}"
                    )),
                }
            }
        }
    }

    /// Shared implementation for [`register`] and [`register_with_fallback`].
    ///
    /// The callback is stored only after the OS registration succeeded, so a
    /// failed attempt does not consume the process-wide handler.
    fn try_register(
        spec: &str,
        on_press: &Arc<dyn Fn() + Send + Sync + 'static>,
    ) -> Result<HotkeyHandle> {
        let label = hotkey_label(spec);
        let normalized = normalize_hotkey_spec(spec);
        let hotkey = parse_hotkey(&normalized, spec)?;

        let manager = GlobalHotKeyManager::new().context(
            "could not create the global hotkey manager; it must be created on \
             the thread that owns the Win32 message loop (a Bevy Startup system)",
        )?;
        manager
            .register(hotkey)
            .with_context(|| format!("could not register hotkey {label:?} ({normalized:?})"))?;

        if ON_PRESS.set(Arc::clone(on_press)).is_err() {
            log::warn!(
                "a global hotkey press callback is already installed; \
                 keeping the existing callback (the press flag is still updated)"
            );
        }
        CURRENT_HOTKEY_ID.store(hotkey.id(), Ordering::Release);
        PRESSED.store(false, Ordering::Release);
        install_event_handler_once();

        Ok(HotkeyHandle {
            manager,
            hotkey,
            spec: label,
        })
    }

    /// Parses a normalised spec into a [`HotKey`].
    fn parse_hotkey(normalized: &str, original: &str) -> Result<HotKey> {
        normalized.parse::<HotKey>().map_err(|error| {
            anyhow!("invalid hotkey {original:?} (normalised to {normalized:?}): {error}")
        })
    }

    /// Installs the process-wide hotkey event handler exactly once.
    ///
    /// `GlobalHotKeyEvent::set_event_handler` stores its argument in a
    /// `OnceCell`, so repeated calls after the first are no-ops; the explicit
    /// call here documents that contract at the call site.
    fn install_event_handler_once() {
        GlobalHotKeyEvent::set_event_handler(Some(handle_hotkey_event));
    }

    /// Process-wide press handler. Runs on the Win32 message-loop thread.
    ///
    /// It is intentionally tiny: it only records the press and forwards the
    /// wake-up to the callback supplied at registration time. In particular it
    /// must not run Bevy systems or block.
    fn handle_hotkey_event(event: GlobalHotKeyEvent) {
        if event.state != HotKeyState::Pressed {
            return;
        }
        if event.id != CURRENT_HOTKEY_ID.load(Ordering::Acquire) {
            return;
        }
        PRESSED.store(true, Ordering::Release);
        if let Some(on_press) = ON_PRESS.get() {
            on_press();
        }
    }
}

#[cfg(not(windows))]
mod imp {
    //! Non-Windows stubs with the same signatures as the Windows backend.

    use std::marker::PhantomData;

    use anyhow::Result;

    /// Stub handle. Exists so the app can be compiled on Linux; it is never
    /// constructed because [`register`] always fails there.
    ///
    /// The `PhantomData<*const ()>` keeps the type `!Send` on every platform so
    /// code that compiles on Linux cannot accidentally rely on sending it (the
    /// real Windows handle is not `Send` either).
    pub struct HotkeyHandle {
        spec: String,
        _not_send: PhantomData<*const ()>,
    }

    impl std::fmt::Debug for HotkeyHandle {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("HotkeyHandle")
                .field("spec", &self.spec)
                .finish_non_exhaustive()
        }
    }

    impl HotkeyHandle {
        /// Always `false`: the stub never sees hotkey presses.
        pub fn pressed(&self) -> bool {
            false
        }

        /// The spec the (never created) binding would use.
        pub fn spec(&self) -> &str {
            &self.spec
        }

        /// Always returns an unsupported error.
        pub fn rebind(&mut self, _spec: &str) -> Result<()> {
            Err(unsupported("global hotkeys"))
        }
    }

    /// Builds a clear "not implemented on this platform" error.
    fn unsupported(feature: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "{feature} are only supported on Windows; this {} build has no hotkey backend",
            super::super::platform_name()
        )
    }

    /// Always returns an unsupported error on non-Windows platforms.
    pub fn register(
        _spec: &str,
        _on_press: impl Fn() + Send + Sync + 'static,
    ) -> Result<HotkeyHandle> {
        Err(unsupported("global hotkeys"))
    }

    /// Always returns an unsupported error on non-Windows platforms.
    pub fn register_with_fallback(
        _spec: &str,
        _on_press: impl Fn() + Send + Sync + 'static,
    ) -> Result<(HotkeyHandle, String, Option<String>)> {
        Err(unsupported("global hotkeys"))
    }
}

pub use self::imp::{HotkeyHandle, register, register_with_fallback};

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SPEC, FALLBACK_SPEC, hotkey_label, hotkey_label_token, normalize_hotkey_spec,
        normalize_hotkey_token, supported,
    };

    #[test]
    fn normalizes_the_default_chord() {
        assert_eq!(normalize_hotkey_spec(DEFAULT_SPEC), "super+shift+tab");
        assert_eq!(
            normalize_hotkey_spec("Win + Shift + Tab"),
            "super+shift+tab"
        );
        assert_eq!(
            normalize_hotkey_spec("WINDOWS+SHIFT+TAB"),
            "super+shift+tab"
        );
    }

    #[test]
    fn normalizes_the_fallback_chord() {
        assert_eq!(normalize_hotkey_spec(FALLBACK_SPEC), "ctrl+shift+tab");
    }

    #[test]
    fn normalisation_is_idempotent() {
        for spec in [
            "Win+Shift+Tab",
            "ctrl+shift+tab",
            "Alt+F4",
            "super+keya",
            "  win  +  esc ",
            "",
        ] {
            let once = normalize_hotkey_spec(spec);
            assert_eq!(normalize_hotkey_spec(&once), once, "spec {spec:?}");
        }
    }

    #[test]
    fn normalizes_friendly_aliases() {
        assert_eq!(normalize_hotkey_spec("Ctrl+Esc"), "ctrl+escape");
        assert_eq!(normalize_hotkey_spec("Alt+PgUp"), "alt+pageup");
        assert_eq!(normalize_hotkey_spec("Shift+Del"), "shift+delete");
        assert_eq!(normalize_hotkey_spec("Win+Space"), "super+space");
        assert_eq!(normalize_hotkey_spec("Meta+Tab"), "super+tab");
    }

    #[test]
    fn normalisation_drops_empty_tokens() {
        assert_eq!(
            normalize_hotkey_spec(" Win + + Shift + Tab "),
            "super+shift+tab"
        );
        assert_eq!(normalize_hotkey_spec("+++"), "");
        assert_eq!(normalize_hotkey_spec("   "), "");
    }

    #[test]
    fn tokens_are_lowercased_and_aliased() {
        // `normalize_hotkey_spec` trims before calling this helper.
        assert_eq!(normalize_hotkey_token("Shift"), "shift");
        assert_eq!(normalize_hotkey_token("CTRL"), "ctrl");
        assert_eq!(normalize_hotkey_token("Win"), "super");
        assert_eq!(normalize_hotkey_token("KeyA"), "keya");
    }

    #[test]
    fn labels_are_human_readable() {
        assert_eq!(hotkey_label("super+shift+tab"), "Win+Shift+Tab");
        assert_eq!(hotkey_label("ctrl+shift+tab"), "Ctrl+Shift+Tab");
        assert_eq!(hotkey_label(DEFAULT_SPEC), "Win+Shift+Tab");
        assert_eq!(hotkey_label(FALLBACK_SPEC), "Ctrl+Shift+Tab");
        assert_eq!(hotkey_label("super+keya"), "Win+A");
        assert_eq!(hotkey_label("ctrl+f5"), "Ctrl+F5");
        assert_eq!(hotkey_label("alt+digit1"), "Alt+1");
        assert_eq!(hotkey_label("ctrl+esc"), "Ctrl+Esc");
    }

    #[test]
    fn label_round_trips_through_normalisation() {
        for spec in ["Win+Shift+Tab", "Ctrl+Shift+Tab", "Alt+F4", "super+keya"] {
            let label = hotkey_label(spec);
            // Labels are canonical display forms, so re-normalising them and
            // formatting again must be a no-op.
            assert_eq!(hotkey_label(&normalize_hotkey_spec(&label)), label);
        }
    }

    #[test]
    fn label_tokens_handle_unknowns() {
        assert_eq!(hotkey_label_token("bogus"), "Bogus");
        assert_eq!(hotkey_label_token("tab"), "Tab");
        assert_eq!(hotkey_label_token("keyz"), "Z");
    }

    #[test]
    fn empty_specs_have_empty_labels() {
        assert_eq!(hotkey_label(""), "");
        assert_eq!(hotkey_label("  "), "");
    }

    #[test]
    fn support_flag_matches_target() {
        assert_eq!(supported(), cfg!(windows));
    }
}
