//! What the Settings window shows.

use super::*;

impl Broom {
    /// `window` is the Settings window, asked only which field has focus.
    pub(super) fn render_settings(&self, s: &SettingsState, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = div()
            .id("settings-card")
            .flex()
            .flex_col()
            .gap_3();

        // Salvaged-files notice: shown once if the last session's settings
        // were damaged and moved aside. Handed to this session when it opened.
        if let Some(ref salvaged) = s.salvaged {
            card = card.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(theme::alpha(colors().review, 0.12))
                    .border_1()
                    .border_color(theme::alpha(colors().review, 0.35))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_xs().text_color(rgb(colors().review)).child(
                        "Your settings file was damaged. It has been moved aside; Broom started with defaults.",
                    ))
                    .child(
                        div().text_xs().text_color(rgb(colors().muted)).child(format!("Location: {}", salvaged.display())),
                    ),
            );
        }

        // --- General -----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("General")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Show menu-bar 🧹 icon"))
                    .child(
                        div()
                            .id("toggle-menubar")
                            .rounded(theme::radius(6.))
                            .px_2()
                            .py_0p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(self.settings.menubar, |d| {
                                d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent))
                            })
                            .when(!self.settings.menubar, |d| {
                                d.bg(rgb(colors().card_hover)).text_color(rgb(colors().muted))
                            })
                            .hover(|d| d.text_color(rgb(colors().text)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_menubar(cx);
                            }))
                            .child(if self.settings.menubar { "On" } else { "Off" }),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().child("Show date modified"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors().muted))
                                    .child("A column in the list, whatever it's sorted by"),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-modified")
                            .rounded(theme::radius(6.))
                            .px_2()
                            .py_0p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(self.settings.show_modified, |d| {
                                d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent))
                            })
                            .when(!self.settings.show_modified, |d| {
                                d.bg(rgb(colors().card_hover)).text_color(rgb(colors().muted))
                            })
                            .hover(|d| d.text_color(rgb(colors().text)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.settings.show_modified;
                                this.set_show_modified(next, cx);
                            }))
                            .child(if self.settings.show_modified { "On" } else { "Off" }),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().child("Show date created"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors().muted))
                                    .child("When each item first appeared on this disk"),
                            ),
                    )
                    .child(
                        div()
                            .id("toggle-created")
                            .rounded(theme::radius(6.))
                            .px_2()
                            .py_0p5()
                            .text_xs()
                            .cursor_pointer()
                            .when(self.settings.show_created, |d| {
                                d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent))
                            })
                            .when(!self.settings.show_created, |d| {
                                d.bg(rgb(colors().card_hover)).text_color(rgb(colors().muted))
                            })
                            .hover(|d| d.text_color(rgb(colors().text)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = !this.settings.show_created;
                                this.set_show_created(next, cx);
                            }))
                            .child(if self.settings.show_created { "On" } else { "Off" }),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Colour chart by"))
                    .child(
                        div().flex().gap_1().children(
                            ["Folder", "Kind"].iter().enumerate().map(|(ix, &label)| {
                                let active = match (self.color_by, self.settings.color_by) {
                                    (_, settings::ColorBy::Folder) => label == "Folder",
                                    (_, settings::ColorBy::Kind) => label == "Kind",
                                };
                                let target = match label {
                                    "Kind" => ColorBy::Kind,
                                    _ => ColorBy::Folder,
                                };
                                div()
                                    .id(("color-by", ix))
                                    .rounded(theme::radius(6.))
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent)))
                                    .when(!active, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_color_by(target, cx)))
                                    .child(label)
                            }),
                        ),
                    ),
            );

        // --- Appearance --------------------------------------------------------
        let chip = |id: (&'static str, usize), label: &'static str, active: bool| {
            div()
                .id(id)
                .rounded(theme::radius(6.))
                .px_2()
                .py_0p5()
                .text_xs()
                .cursor_pointer()
                .when(active, |d| d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent)))
                .when(!active, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                .child(label)
        };
        let has_light = self.settings.theme.has_light();
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Appearance")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Theme"))
                    .child(
                        div().flex().gap_1().children(theme::ThemeName::ALL.iter().enumerate().map(|(ix, &name)| {
                            chip(("theme", ix), name.label(), self.settings.theme == name)
                                .on_click(cx.listener(move |this, _, _, cx| this.set_theme(name, cx)))
                        })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().child("Light or dark"))
                            .when(!has_light, |d| {
                                d.child(div().text_xs().text_color(rgb(colors().muted)).child("Classic is dark only"))
                            }),
                    )
                    .child(
                        div().flex().gap_1().when(!has_light, |d| d.opacity(0.4)).children(
                            theme::Appearance::ALL.iter().enumerate().map(|(ix, &mode)| {
                                chip(("appearance", ix), mode.label(), self.settings.appearance == mode)
                                    .when(has_light, |d| {
                                        d.on_click(cx.listener(move |this, _, _, cx| this.set_appearance(mode, cx)))
                                    })
                                    .when(!has_light, |d| d.cursor_default())
                            }),
                        ),
                    ),
            );

        // --- Scanning ----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Scanning")),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Outline depth"))
                    .child(
                        div().flex().gap_1().children(
                            (0..=4).map(|d| {
                                let active = self.settings.outline_depth == d;
                                div()
                                    .id(("outline-depth", d))
                                    .rounded(theme::radius(6.))
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent)))
                                    .when(!active, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.outline_depth = d;
                                        let _ = settings::save(&this.settings);
                                        cx.notify();
                                    }))
                                    .child(d.to_string())
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(rgb(colors().card))
                    .border_1()
                    .border_color(rgb(colors().border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child("Banner duration (seconds)"))
                    .child(
                        div().flex().gap_1().children(
                            [4usize, 8, 15, 30].iter().map(|&s| {
                                let active = self.settings.banner_seconds == s as u64;
                                div()
                                    .id(("banner-sec", s))
                                    .rounded(theme::radius(6.))
                                    .px_2()
                                    .py_0p5()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(active, |d| d.bg(rgb(colors().accent)).text_color(rgb(colors().on_accent)))
                                    .when(!active, |d| d.text_color(rgb(colors().muted)).hover(|s| s.text_color(rgb(colors().text))))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.banner_seconds = s as u64;
                                        let _ = settings::save(&this.settings);
                                        cx.notify();
                                    }))
                                    .child(format!("{s}"))
                            }),
                        ),
                    ),
            );

        // --- Excluded folders ---------------------------------------------------
        card = card.child(
            div()
                .px_3()
                .py_2()
                .rounded(theme::radius(8.))
                .bg(rgb(colors().card))
                .border_1()
                .border_color(rgb(colors().border))
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().child("Excluded folders"))
                .child(
                    div().flex().flex_col().gap_1().children(
                        self.settings.excludes.iter().enumerate().map(|(ix, path)| {
                            let path = path.clone();
                            div()
                                .id(("exclude-item", ix))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(div().text_xs().font_family(theme::current().mono).truncate().child(path.clone()))
                                .child(
                                    div()
                                        .id(("exclude-remove", ix))
                                        .text_xs()
                                        .text_color(rgb(colors().danger))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.settings.excludes.retain(|p| *p != path);
                                            let _ = settings::save(&this.settings);
                                            cx.notify();
                                        }))
                                        .child("✕"),
                                )
                        }),
                    ),
                )
                .child(
                    div().flex().gap_2().items_center().child(
                        div()
                            .flex_1()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                text_input("exclude-path")
                                    .placeholder("e.g. ~/Downloads")
                                    .placeholder_color(theme::overlay(0.5))
                                    .caret_color(theme::input_caret())
                                    .selection_color(theme::input_selection())
                                    .state(s.exclude_input.downgrade())
                                    .caret_blink_interval_500ms()
                                    .w_full()
                                    .h(px(28.))
                                    .px_2()
                                    .min_h_auto()
                                    .rounded(theme::radius(6.))
                                    .border_1()
                                    .border_color(field_border(&s.exclude_input, window, cx))
                                    .bg(rgb(colors().bg))
                                    .text_size(px(12.))
                                    // One line as tall as the field inside its border: the element draws
                                    // from the top, so this, not flex, is what centres the text.
                                    .line_height(px(26.)),
                            ),
                    )
                    .child(
                        button("exclude-add", "Add").on_click(cx.listener(|this, _, _, cx| {
                            let text = this.prefs.as_ref().and_then(|p| {
                                let input = p.settings.exclude_input.read(cx);
                                let text = input.as_str();
                                if text.trim().is_empty() { None } else { Some(text.to_string()) }
                            });
                            if let Some(path) = text {
                                if !this.settings.excludes.contains(&path) {
                                    this.settings.excludes.push(path);
                                    let _ = settings::save(&this.settings);
                                }
                                if let Some(p) = &mut this.prefs {
                                    p.settings.exclude_input.update(cx, |input, cx| input.emplace("", cx));
                                }
                                cx.notify();
                            }
                        })),
                    ),
                ),
        );

        // --- API keys ----------------------------------------------------------
        card = card
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("API keys"))
                    .child(
                        div().text_xs().text_color(rgb(colors().muted)).child(
                            "Stored in your login keychain. Broom shows only the last characters — never the key itself.",
                        ),
                    )
                    .child(
                        div().text_xs().text_color(rgb(colors().muted)).child(
                            "A fallback reader needs no key — it answers whenever the main one can't.",
                        ),
                    ),
            );
        for (ix, row) in s.rows.iter().enumerate() {
            card = card.child(self.render_key_row(ix, row, cx));
        }
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(rgb(colors().muted)).child("Check another key name:"))
                .child(
                    div().flex().gap_2().items_center().child(
                        div()
                            .flex_1()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                text_input("custom-key")
                                    .placeholder("e.g. openai")
                                    .placeholder_color(theme::overlay(0.5))
                                    .caret_color(theme::input_caret())
                                    .selection_color(theme::input_selection())
                                    .state(s.custom.downgrade())
                                    .caret_blink_interval_500ms()
                                    .w_full()
                                    .h(px(28.))
                                    .px_2()
                                    .min_h_auto()
                                    .rounded(theme::radius(6.))
                                    .border_1()
                                    .border_color(field_border(&s.custom, window, cx))
                                    .bg(rgb(colors().bg))
                                    .text_size(px(12.))
                                    // One line as tall as the field inside its border: the element draws
                                    // from the top, so this, not flex, is what centres the text.
                                    .line_height(px(26.)),
                            ),
                    )
                    .child(
                        button("check-key", "Check").on_click(cx.listener(|this, _, _, cx| this.add_custom_key(cx))),
                    ),
                ),
        );
        if let Some(note) = &s.note {
            card = card.child(div().text_xs().text_color(rgb(colors().muted)).child(note.clone()));
        }
        card
    }
}
