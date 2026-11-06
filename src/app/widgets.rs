//! Buttons, checkboxes, meters and other small pieces of UI.

use super::*;

pub(super) fn button_base(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(theme::radius(6.))
        .border_1()
        .cursor_pointer()
        .active(|s| s.opacity(0.8))
        // Keep clicks from starting a window drag in the toolbar.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label.into())
}

pub(super) fn button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<gpui::Div> {
    button_base(id, label)
        .bg(rgb(colors().card))
        .border_color(rgb(colors().border))
        .hover(|s| s.bg(rgb(colors().card_hover)))
}

pub(super) fn primary_button(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, color: u32) -> Stateful<gpui::Div> {
    button_base(id, label)
        .bg(rgb(color))
        .border_color(rgb(color))
        .text_color(rgb(theme::on(color)))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.bg(lighten(rgb(color))))
}

pub(super) fn lighten(color: Rgba) -> Hsla {
    let mut hsla: Hsla = color.into_color();
    hsla.lightness = (hsla.lightness + 0.07).min(1.0);
    hsla
}

pub(super) fn to_hsla(color: u32) -> Hsla {
    rgb(color).into_color()
}

/// Clicking outside an open context menu closes it; the menu stops this handler
/// for clicks inside itself.
pub(super) fn close_menu_on_outside(this: &mut Broom, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Broom>) {
    if this.context_menu.take().is_some() {
        cx.notify();
    }
}

/// A staging checkbox. Clicks are wired by the caller; `enabled` is false for
/// things that can't be staged (the root, home folders, volume slices).
pub(super) fn checkbox(id: impl Into<gpui::ElementId>, tick: Tick, enabled: bool) -> Stateful<gpui::Div> {
    let accent = to_hsla(colors().accent);
    let base = div()
        .id(id)
        .size(px(15.))
        .flex_none()
        .rounded(theme::radius(4.))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    let check = || div().text_color(rgb(colors().on_accent)).text_size(px(10.)).font_weight(FontWeight::BOLD).child("✓");
    let base = match tick {
        Tick::Off => base.border_color(rgb(colors().faint)).bg(rgb(colors().bg)).when(enabled, |d| d.hover(|s| s.border_color(rgb(colors().accent)))),
        Tick::On => base.border_color(accent).bg(accent).child(check()),
        Tick::Implied => base
            .border_color(gpui::hsla(accent.hue.into_positive_degrees() / 360.0, accent.saturation, accent.lightness, 0.5))
            .bg(gpui::hsla(accent.hue.into_positive_degrees() / 360.0, accent.saturation, accent.lightness, 0.45))
            .child(check()),
        Tick::Mixed => base.border_color(accent).bg(rgb(colors().bg)).child(div().w(px(7.)).h(px(2.)).rounded_full().bg(accent)),
    };
    base.when(enabled && tick != Tick::Implied, |d| d.cursor_pointer()).when(!enabled, |d| d.opacity(0.3))
}

/// What a row is, as a mark rather than an emoji: a filled square for a folder,
/// an outlined one for a file, a grey one for an APFS slice.
pub(super) fn kind_mark(kind: Kind, swatch: Hsla) -> impl IntoElement {
    let mark = div().size(px(11.)).flex_none().rounded(theme::radius(4.));
    match kind {
        Kind::Dir => mark.bg(swatch),
        Kind::File => mark.border_1().border_color(swatch).bg(gpui::hsla(swatch.hue.into_positive_degrees() / 360.0, swatch.saturation, swatch.lightness, 0.25)),
        Kind::Other => mark.bg(rgb(colors().faint)),
    }
}

/// The plan's stand-in before there are results.
pub(super) fn plan_placeholder(text: &'static str) -> impl IntoElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(rgb(colors().border))
        .child(div().px_3().pt_3().pb_1p5().font_weight(FontWeight::SEMIBOLD).child("Cleanup plan"))
        .child(div().px_3().text_xs().text_color(rgb(colors().muted)).child(text))
}

/// Shape a chart label that fits in `max_width`, truncating with an ellipsis;
/// `None` when not even two characters fit.
pub(super) fn fit_label(window: &Window, text: &str, max_width: f32, font: &gpui::Font, size: Pixels) -> Option<gpui::ShapedLine> {
    let color = gpui::hsla(0., 0., 0.08, 0.92);
    let shape = |t: SharedString| {
        let run = TextRun { len: t.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None, letter_spacing: None };
        window.text_system().shape_line(t, size, &[run], None)
    };
    let full = shape(text.to_string().into());
    if f32::from(full.width()) <= max_width {
        return Some(full);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len().saturating_sub(1);
    while n >= 2 {
        let mut t: String = chars[..n].iter().collect();
        t.push('…');
        let line = shape(t.into());
        if f32::from(line.width()) <= max_width {
            return Some(line);
        }
        n -= 1;
    }
    None
}

/// Hover a path that may be cut short and the whole of it turns up in a
/// caption box.
pub(super) fn path_tip(el: Stateful<gpui::Div>, path: String) -> Stateful<gpui::Div> {
    let path: SharedString = path.into();
    el.tooltip(move |_, cx| cx.new(|_| comic::PathCaption(path.clone())).into())
}

pub(super) fn icon_button(id: impl Into<gpui::ElementId>, glyph: icons::Symbol, tooltip: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::radius(6.))
        .text_color(rgb(colors().muted))
        .cursor_pointer()
        .tooltip(move |_, cx| cx.new(|_| Tip(tooltip.into())).into())
        .hover(|s| s.bg(rgb(colors().border)).text_color(rgb(colors().text)))
        .child(icons::inline_symbol(glyph, 13.))
}

/// The app icon, as the About card's brand mark (a 256 px copy of packaging/AppIcon.png).
pub(super) static ABOUT_MARK: LazyLock<Arc<Image>> =
    LazyLock::new(|| Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../../packaging/AppIcon-256.png").to_vec())));

/// A volume over 90% full reads as danger whatever its hue.
pub(super) fn level_color(fraction: f32, base: Hsla) -> Hsla {
    if fraction > 0.9 {
        to_hsla(colors().danger)
    } else {
        base
    }
}

pub(super) fn meter(fraction: f32, fill: Hsla) -> impl IntoElement {
    let color = level_color(fraction, fill);
    div()
        .mt_1()
        .h(px(6.))
        .w_full()
        .rounded_full()
        // Translucent, like the card it sits on: a solid dark track would
        // read as a stray line over glass.
        .bg(theme::overlay(0.12))
        .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded_full().bg(color))
}
