//! Storing and editing API keys in the Keychain.

use super::*;

impl Broom {
    /// A transient one-line status inside Settings ("saved", "couldn't…").
    pub(super) fn set_note(&mut self, text: impl Into<SharedString>) {
        if let Some(p) = &mut self.prefs {
            p.settings.note = Some(text.into());
        }
    }

    /// Re-read the keychain for one row, off the UI thread, and update its status.
    /// The epoch drops results from a Settings session that has since closed and
    /// been reopened.
    pub(super) fn refresh_key_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(service) = self.prefs.as_ref().and_then(|p| p.settings.rows.get(ix).map(|r| r.service.to_string())) else { return };
        if let Some(row) = self.prefs.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Loading;
        }
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let key = cx.background_spawn(async move { secrets::read(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                if let Some(row) = this.prefs.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
                    row.status = KeyStatus::of(key);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Arm a row for pasting: the next clipboard paste goes straight to the keychain.
    pub(super) fn edit_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.prefs.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            if matches!(row.status, KeyStatus::Missing | KeyStatus::Set { .. }) {
                row.status = KeyStatus::Editing;
            }
        }
        cx.notify();
    }

    /// Take whatever text is on the clipboard as the row's new key. Nothing is
    /// rendered: the value goes from the pasteboard into the keychain directly,
    /// never through a visible field.
    pub(super) fn paste_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        let service = match &self.prefs {
            Some(p) => match p.settings.rows.get(ix).map(|r| (r.service.to_string(), &r.status)) {
                Some((service, KeyStatus::Editing)) => service,
                _ => return,
            },
            None => return,
        };
        let Some(secret) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_note("the clipboard holds no text \u{2014} copy the key first");
            return;
        };
        let secret = secret.trim().to_string();
        if secret.is_empty() {
            self.set_note("the clipboard holds no text \u{2014} copy the key first");
            return;
        }
        self.store_key(ix, &service, secret, cx);
    }

    /// Write a value into the keychain in the background; the row waits as
    /// `Saving` so its buttons can't fire again mid-write. Errors carry status
    /// text only, never key material.
    pub(super) fn store_key(&mut self, ix: usize, service: &str, secret: String, cx: &mut Context<Self>) {
        if let Some(row) = self.prefs.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Saving;
        }
        self.set_note("saving to the keychain\u{2026}");
        let service = service.to_string();
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let saved = cx.background_spawn(async move { secrets::write(&service, &secret) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                ask::invalidate_keys();
                match saved {
                    Ok(()) => this.set_note(format!("\u{201c}{name}\u{201d} saved to the keychain")),
                    Err(error) => this.set_note(format!("couldn\u{2019}t save \u{201c}{name}\u{201d}: {error}")),
                }
                this.refresh_key_row(ix, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Drop the paste state and go back to whatever the keychain says.
    pub(super) fn cancel_edit_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.refresh_key_row(ix, cx);
    }

    /// Remove a service's value from the keychain.
    pub(super) fn remove_key(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(service) = self.prefs.as_ref().and_then(|p| p.settings.rows.get(ix).map(|r| r.service.to_string())) else { return };
        if let Some(row) = self.prefs.as_mut().and_then(|p| p.settings.rows.get_mut(ix)) {
            row.status = KeyStatus::Saving;
        }
        self.set_note("removing\u{2026}");
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let removed = cx.background_spawn(async move { secrets::delete(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                ask::invalidate_keys();
                match removed {
                    Ok(()) => this.set_note(format!("\u{201c}{name}\u{201d} removed from the keychain")),
                    Err(error) => this.set_note(format!("couldn\u{2019}t remove \u{201c}{name}\u{201d}: {error}")),
                }
                this.refresh_key_row(ix, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Look a service name up in the keychain and add it as a row, masked. The
    /// note is epoch-tagged so a slow read for an older name can't overwrite the
    /// answer to the one the user just asked about.
    pub(super) fn add_custom_key(&mut self, cx: &mut Context<Self>) {
        let service = match &self.prefs {
            Some(p) => p.settings.custom.read(cx).as_str().trim().to_string(),
            None => return,
        };
        if service.is_empty() {
            self.set_note("type a key name first");
            return;
        }
        let note_epoch = match &mut self.prefs {
            Some(p) => {
                let s = &mut p.settings;
                if s.rows.iter().any(|r| r.service.as_ref() == service) {
                    s.note = Some(format!("\u{201c}{service}\u{201d} is already listed").into());
                    cx.notify();
                    return;
                }
                s.note_epoch += 1;
                s.custom.update(cx, |state, cx| state.emplace("", cx));
                s.rows.push(KeyRow {
                    title: service.clone().into(),
                    service: service.clone().into(),
                    hint: "Any keychain key, watched here.".into(),
                    status: KeyStatus::Loading,
                });
                s.note_epoch
            }
            None => return,
        };
        let ix = match &self.prefs {
            Some(p) => p.settings.rows.len() - 1,
            None => return,
        };
        let epoch = self.settings_epoch;
        cx.spawn(async move |this, cx| {
            let name = service.clone();
            let key = cx.background_spawn(async move { secrets::read(&service) }).await;
            this.update(cx, |this, cx| {
                if this.settings_epoch != epoch {
                    return;
                }
                if let Some(p) = &mut this.prefs {
                    let s = &mut p.settings;
                    if s.note_epoch == note_epoch {
                        s.note = Some(match &key {
                            Some(_) => format!("\u{201c}{name}\u{201d} found in the keychain"),
                            None => format!("no \u{201c}{name}\u{201d} in the keychain \u{2014} press Paste to add one"),
                        }.into());
                        if let Some(row) = s.rows.get_mut(ix) {
                            row.status = KeyStatus::of(key);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_key_row(&self, ix: usize, row: &KeyRow, cx: &mut Context<Self>) -> impl IntoElement {
        // Env keys outrank the keychain; say so when one is set.
        let hint = match row.service.as_ref() {
            secrets::JEV_SERVICE
                if std::env::var_os("TYPESAFE_API_KEY").or_else(|| std::env::var_os("JEV_API_KEY")).is_some() =>
                format!("{} An env key is set and takes precedence.", row.hint),
            _ => row.hint.to_string(),
        };
        let mut head = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(row.title.clone()))
                    .child(div().text_xs().text_color(rgb(colors().muted)).truncate().child(hint)),
            );
        match &row.status {
            KeyStatus::Loading => {
                head = head.child(div().text_xs().text_color(rgb(colors().muted)).child("checking keychain…"));
            }
            KeyStatus::Missing => {
                head = head.child(
                    button(("key-add", ix), "Add")
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_key(ix, cx))),
                );
            }
            KeyStatus::Set { masked } => {
                head = head
                    .child(div().text_xs().font_family(theme::current().mono).text_color(rgb(colors().muted)).child(masked.clone()))
                    .child(
                        button(("key-edit", ix), "Update")
                            .on_click(cx.listener(move |this, _, _, cx| this.edit_key(ix, cx))),
                    )
                    .child(
                        button(("key-remove", ix), "Remove")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_key(ix, cx))),
                    );
            }
            KeyStatus::Editing | KeyStatus::Saving => {}
        }
        let mut line = div()
            .id(("key-row", ix))
            .px_3()
            .py_2()
            .rounded(theme::radius(8.))
            .bg(rgb(colors().card))
            .border_1()
            .border_color(rgb(colors().border))
            .flex()
            .flex_col()
            .gap_1()
            .child(head);
        if matches!(row.status, KeyStatus::Editing) {
            // No text field: the key is pasted from the clipboard straight into
            // the keychain, so it is never displayed.
            line = line.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(theme::radius(8.))
                    .bg(theme::alpha(colors().accent, 0.12))
                    .border_1()
                    .border_color(theme::alpha(colors().accent, 0.35))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div().text_xs().text_color(rgb(colors().accent)).child(
                            "1. Copy the API key to your clipboard  (⌘C)",
                        ),
                    )
                    .child(
                        div().text_xs().text_color(rgb(colors().accent)).child(
                            "2. Click \"Paste from Clipboard\" below — Broom reads it from there, never shows it on screen.",
                        ),
                    )
                    .child(
                        div().flex().gap_2().items_center().justify_end().child(
                            primary_button(("key-paste", ix), "Paste from Clipboard", colors().accent)
                                .on_click(cx.listener(move |this, _, _, cx| this.paste_key(ix, cx))),
                        )
                        .child(
                            button(("key-cancel", ix), "Cancel")
                                .on_click(cx.listener(move |this, _, _, cx| this.cancel_edit_key(ix, cx))),
                        ),
                    ),
            );
        }
        if matches!(row.status, KeyStatus::Saving) {
            line = line.child(
                div().text_xs().text_color(rgb(colors().muted)).child("working in the keychain\u{2026}"),
            );
        }
        line
    }
}
