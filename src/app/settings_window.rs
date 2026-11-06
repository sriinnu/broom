//! Settings in a window of its own, as the Human Interface Guidelines have
//! it: titled "Settings", not modal, opened or brought forward by ⌘, and
//! closed by ⌘W, Escape or its close button.
//!
//! The window is a thin view. Everything it shows, and every change it
//! makes, belongs to `Broom`: its render calls into Broom's, and the
//! listeners in there are Broom's own, so a change lands in the main window
//! the moment it's made, exactly as it did from the old card.

use super::*;

pub struct SettingsWindow {
    broom: Entity<Broom>,
    /// The window's root focus, so its key context ("Settings") is on the
    /// dispatch path and Escape finds the close binding.
    focus: FocusHandle,
    /// Repaint whenever Broom does: a keychain answer, a theme change.
    _observe: Subscription,
}

impl SettingsWindow {
    fn new(broom: Entity<Broom>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let _observe = cx.observe(&broom, |_, _, cx| cx.notify());
        // However the window goes (close button, ⌘W, Escape), its state goes
        // with it, so the next ⌘, starts fresh from the keychain. The id keeps
        // a late release from clearing a newer window's state.
        let id = window.window_handle().window_id();
        cx.on_release(move |this, cx| this.broom.update(cx, |b, cx| b.settings_closed(id, cx))).detach();
        Self { broom, focus, _observe }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.broom.update(cx, |b, cx| b.render_settings_window(window, cx));
        div()
            .id("settings-window")
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            // Escape and Enter inside a field come here as the query field's
            // actions (their bindings sit at text-field depth): Escape closes,
            // Enter checks a key name, as in the old card.
            .on_action(|_: &DismissQuery, window, _| window.remove_window())
            .on_action(cx.listener(|this, _: &SubmitQuery, window, cx| {
                this.broom.update(cx, |b, cx| b.settings_enter(window, cx));
            }))
            // ⌘, with Settings already in front: nothing more to open.
            .on_action(|_: &OpenSettings, window, _| window.activate())
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(colors().panel))
            .text_color(rgb(colors().text))
            .font_family(theme::current().font)
            .text_sm()
            .child(content)
    }
}

fn settings_window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(560.), px(640.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: false, traffic_light_position: None }),
        // A settings window isn't something to park in the Dock.
        is_minimizable: false,
        // Taller or shorter to suit the screen; the content scrolls.
        window_min_size: Some(size(px(480.), px(320.))),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    }
}

/// ⌘, the app menu's Settings… and the toolbar button: bring the Settings
/// window forward, opening it if it isn't open. An App-level function, not
/// a Broom listener, because the new window's first render reads Broom.
pub fn open_settings_window(cx: &mut App) {
    let Some(broom) = broom() else { return };
    let open = broom.read(cx).prefs.as_ref().and_then(|p| p.window);
    if let Some(handle) = open {
        if handle.update(cx, |_, window, _| window.activate()).is_ok() {
            return;
        }
    }
    broom.update(cx, |b, cx| b.begin_settings(cx));
    let opened = cx.open_window(settings_window_options(cx), |window, cx| {
        let broom = broom.clone();
        cx.new(|cx| SettingsWindow::new(broom, window, cx))
    });
    match opened {
        Ok(handle) => broom.update(cx, |b, _| {
            if let Some(p) = &mut b.prefs {
                p.window = Some(handle);
            }
        }),
        // No window, no state: the keychain reads have nowhere to land.
        Err(_) => broom.update(cx, |b, cx| {
            b.prefs = None;
            cx.notify();
        }),
    }
}

/// Dev aid (`snapshot` feature): the Settings window's pixels, if it's open.
/// Its own window, so the recorder's `shot` can't see it.
#[cfg(feature = "snapshot")]
pub fn capture_settings_window(cx: &mut App) -> Option<Result<image::RgbaImage, String>> {
    let handle = broom()?.read(cx).prefs.as_ref()?.window?;
    // The window-level handle: drawing runs the view's render, which must
    // not happen inside the view's own update lease.
    gpui::AnyWindowHandle::from(handle)
        .update(cx, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.render_to_image().map_err(|e| format!("{e:#}"))
        })
        .ok()
}

/// Dev aid (`snapshot` feature): run `f` against the Settings window, for
/// scripted clicks and keys in it.
#[cfg(feature = "snapshot")]
pub fn with_settings_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    let Some(handle) = broom().and_then(|b| b.read(cx).prefs.as_ref().and_then(|p| p.window)) else { return };
    let _ = gpui::AnyWindowHandle::from(handle).update(cx, |_, window, cx| f(window, cx));
}

impl Broom {
    /// A fresh Settings session: key rows read again from the keychain, the
    /// salvage notice (if any) handed over for its one showing.
    pub(super) fn begin_settings(&mut self, cx: &mut Context<Self>) {
        let rows = vec![KeyRow {
            service: secrets::JEV_SERVICE.into(),
            title: "Reader key (typesafe.ai)".into(),
            hint: "Lets the reader judge what an Ask query means.".into(),
            status: KeyStatus::Loading,
        }];
        self.settings_epoch += 1;
        self.prefs = Some(SettingsPane {
            settings: SettingsState {
                rows,
                custom: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                exclude_input: cx.new(|cx| EditableTextState::new(StringStorage::default(), cx)),
                note: None,
                note_epoch: 0,
                salvaged: self.settings_salvaged.take(),
            },
            scroller: Scroller::default(),
            window: None,
        });
        self.refresh_key_row(0, cx);
    }

    /// The Settings window closed. Only that window's state goes; one opened
    /// since keeps its own.
    pub(super) fn settings_closed(&mut self, id: gpui::WindowId, cx: &mut Context<Self>) {
        let ours = self.prefs.as_ref().is_some_and(|p| p.window.is_none_or(|w| w.window_id() == id));
        if ours {
            self.prefs = None;
            cx.notify();
        }
    }

    /// The window's content: the settings in a scroll area, with Broom's
    /// scrollbar beside them — none when it all fits.
    pub(super) fn render_settings_window(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(p) = &self.prefs else { return div().into_any_element() };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap_2()
            .pl_5()
            .pr_2()
            .py_4()
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&p.scroller.scroll)
                    .child(
                        div()
                            .id("settings-content")
                            .flex()
                            .flex_col()
                            .pr_2()
                            .child(measure(p.scroller.content_top.clone()))
                            .child(self.render_settings(&p.settings, window, cx))
                            .child(measure(p.scroller.content_bottom.clone())),
                    ),
            )
            .child(panel_scrollbar(&p.scroller))
            .into_any_element()
    }
}
