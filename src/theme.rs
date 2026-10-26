//! How Broom looks: five themes, each a palette for light and for dark, plus
//! the shape around it (corner radius, fonts, row height).
//!
//! Classic is the look Broom shipped with and stays the default, dark only.
//! Darkroom, Ledger, Console and Bloom come from the HTML prototypes; their
//! colours are the prototypes' CSS tokens, one for one. The layout is the same
//! for all of them: a theme changes what things look like, not where they are.
//!
//! The current theme lives in one static, read on every paint. Only the UI
//! thread writes it (`apply`), then repaints every window, so readers never
//! see half a change.

use std::sync::atomic::{AtomicU8, Ordering};

use gpui::{App, Hsla, Pixels, WindowAppearance, px};
use serde::{Deserialize, Serialize};

/// Which theme, as Settings names it and as the file spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeName {
    #[default]
    Classic,
    Darkroom,
    Ledger,
    Console,
    Bloom,
}

impl ThemeName {
    pub const ALL: [ThemeName; 5] = [Self::Classic, Self::Darkroom, Self::Ledger, Self::Console, Self::Bloom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Darkroom => "Darkroom",
            Self::Ledger => "Ledger",
            Self::Console => "Console",
            Self::Bloom => "Bloom",
        }
    }

    fn theme(self) -> &'static Theme {
        &THEMES[self as usize]
    }

    /// Classic has no light palette; the appearance setting doesn't apply.
    pub fn has_light(self) -> bool {
        self != Self::Classic
    }
}

/// Light, dark, or whatever macOS is set to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    Auto,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Self::Auto, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Every colour the window and the menu-bar panel paint with.
pub struct Palette {
    /// The window behind everything.
    pub bg: u32,
    /// Toolbar, rail and side columns.
    pub panel: u32,
    /// Rows and cards sitting on a panel.
    pub card: u32,
    pub card_hover: u32,
    /// The menu-bar panel's cards, a shade off the window's in Classic.
    pub popover_card: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    /// Unticked checkbox rims and the "other" kind swatch.
    pub faint: u32,
    pub accent: u32,
    /// Text on an accent fill.
    pub on_accent: u32,
    pub danger: u32,
    /// Text on a danger fill.
    pub on_danger: u32,
    /// "Safe to delete" and "review first", shared by findings and chips.
    pub safe: u32,
    pub review: u32,
    pub tooltip: u32,
    /// The disc behind the chart, and its centre (plain and hovered).
    pub chart_surface: u32,
    pub chart_center: u32,
    pub chart_center_hover: u32,
}

pub struct Theme {
    pub name: ThemeName,
    pub dark: Palette,
    pub light: Palette,
    /// The body font, the one for titles, and the one for sizes and paths.
    pub font: &'static str,
    pub display: &'static str,
    pub mono: &'static str,
    /// Multiplies every corner radius; 1.0 is Classic's.
    pub radius: f32,
    pub row_height: f32,
}

/// The categorical ladder the icon, the rail and the chart share: rose, lime,
/// periwinkle, gold, teal, tangerine. Validated to stay apart for every kind
/// of colour vision and to carry dark text. The light-ground set is the same
/// order, a touch deeper so it holds against white.
const HUES_DARK: [u32; 6] = [0xd85164, 0x699f08, 0x5a76e4, 0xbc8900, 0x009c9c, 0xd26c28];
const HUES_LIGHT: [u32; 6] = [0xca4459, 0x5d9300, 0x4c66d3, 0xbc8900, 0x008f90, 0xcb6620];

/// `a` moved `pct` percent of the way to `b`, per channel.
const fn mix(a: u32, b: u32, pct: u32) -> u32 {
    const fn channel(a: u32, b: u32, pct: u32, shift: u32) -> u32 {
        let x = (a >> shift) & 0xff;
        let y = (b >> shift) & 0xff;
        ((x * (100 - pct) + y * pct) / 100) << shift
    }
    channel(a, b, pct, 16) | channel(a, b, pct, 8) | channel(a, b, pct, 0)
}

/// A palette from a prototype's tokens; the in-between shades are mixed from them.
#[allow(clippy::too_many_arguments)]
const fn tokens(
    bg: u32, panel: u32, panel_2: u32, fg: u32, muted: u32, line: u32,
    accent: u32, accent_fg: u32, danger: u32, danger_fg: u32, safe: u32, review: u32,
    chart_surface: u32, chart_center: u32,
) -> Palette {
    Palette {
        bg,
        panel,
        card: panel_2,
        card_hover: mix(panel_2, fg, 8),
        popover_card: panel_2,
        border: line,
        text: fg,
        muted,
        faint: mix(line, muted, 50),
        accent,
        on_accent: accent_fg,
        danger,
        on_danger: danger_fg,
        safe,
        review,
        tooltip: mix(bg, fg, 4),
        chart_surface,
        chart_center,
        chart_center_hover: mix(chart_center, fg, 10),
    }
}

const CLASSIC: Palette = Palette {
    bg: 0x1c1d21,
    panel: 0x232529,
    card: 0x2a2c31,
    card_hover: 0x33363c,
    popover_card: 0x25272d,
    border: 0x34363c,
    text: 0xe8e9ec,
    muted: 0x8d919a,
    faint: 0x5d616b,
    accent: 0x4f9dff,
    on_accent: 0xffffff,
    danger: 0xe5484d,
    on_danger: 0xffffff,
    safe: 0x3fb950,
    review: 0xd29922,
    tooltip: 0x141519,
    chart_surface: 0x18191c,
    chart_center: 0x2b2d33,
    chart_center_hover: 0x3a3d44,
};

static THEMES: [Theme; 5] = [
    Theme {
        name: ThemeName::Classic,
        dark: CLASSIC,
        light: CLASSIC,
        font: ".SystemUIFont",
        display: ".SystemUIFont",
        mono: "Menlo",
        radius: 1.0,
        row_height: 30.0,
    },
    // A dark instrument panel around the chart.
    Theme {
        name: ThemeName::Darkroom,
        dark: tokens(
            0x121317, 0x1a1b20, 0x23252c, 0xe9eaee, 0x8f93a0, 0x2d3038,
            0x8a9dff, 0x0b0c10, 0xe5484d, 0xffffff, 0x52c46d, 0xe3a93a, 0x121317, 0x1e2026,
        ),
        light: tokens(
            0xf2f2f5, 0xffffff, 0xeaebf0, 0x1b1c22, 0x666a77, 0xd9dbe3,
            0x4c66d3, 0xffffff, 0xd23b40, 0xffffff, 0x2f8f4a, 0xa86f0f, 0xf2f2f5, 0xffffff,
        ),
        font: ".SystemUIFont",
        display: ".SystemUIFont",
        mono: "Menlo",
        radius: 1.4,
        row_height: 32.0,
    },
    // Paper, ink and hairline rules; serif titles.
    Theme {
        name: ThemeName::Ledger,
        dark: tokens(
            0x1b1d1c, 0x222523, 0x2c302d, 0xe8e9e3, 0x979d95, 0x3a3f3b,
            0x7fb0ea, 0x0d1a2a, 0xee6f68, 0x1a0a09, 0x6fc47f, 0xe0b24a, 0x222523, 0x1b1d1c,
        ),
        light: tokens(
            0xf3f4f0, 0xfcfcfa, 0xe9ebe4, 0x1d2430, 0x68707e, 0xd3d6cc,
            0x1f5fa8, 0xffffff, 0xb3261e, 0xffffff, 0x2a7a3b, 0x9a6207, 0xfcfcfa, 0xf3f4f0,
        ),
        font: ".SystemUIFont",
        display: "Georgia",
        mono: "Menlo",
        radius: 0.5,
        row_height: 30.0,
    },
    // Telemetry: mono everywhere, amber readouts, square corners.
    Theme {
        name: ThemeName::Console,
        dark: tokens(
            0x0c0f12, 0x11151a, 0x181d24, 0xdfe6ea, 0x7f8b95, 0x242b34,
            0xf2b544, 0x14100a, 0xff5c61, 0x1a0608, 0x45d47f, 0xf2b544, 0x0c0f12, 0x11151a,
        ),
        light: tokens(
            0xe9ecef, 0xf6f7f9, 0xdfe3e8, 0x141a20, 0x5d6873, 0xc4cbd3,
            0x9a6400, 0xffffff, 0xc52f35, 0xffffff, 0x1f8a4c, 0x9a6400, 0xf6f7f9, 0xe9ecef,
        ),
        font: "Menlo",
        display: "Menlo",
        mono: "Menlo",
        radius: 0.3,
        row_height: 28.0,
    },
    // Soft, rounded, blossom-coloured.
    Theme {
        name: ThemeName::Bloom,
        dark: tokens(
            0x18151f, 0x221e2b, 0x2c2738, 0xefeaf6, 0x9a93a8, 0x352f44,
            0x8c9dff, 0x0f1030, 0xff6b8a, 0x2a0a12, 0x6fcf7f, 0xe6b35a, 0x221e2b, 0x18151f,
        ),
        light: tokens(
            0xf7f5fa, 0xffffff, 0xf0ecf5, 0x2a2438, 0x7b7489, 0xe4dfeb,
            0x5a76e4, 0xffffff, 0xd14060, 0xffffff, 0x3f9a4f, 0xb8720f, 0xffffff, 0xf7f5fa,
        ),
        font: "Avenir Next",
        display: "Avenir Next",
        mono: "Menlo",
        radius: 2.0,
        row_height: 34.0,
    },
];

/// Theme index in the high bits, "light" in bit 0. Classic, dark, until `apply`.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> &'static Theme {
    &THEMES[(CURRENT.load(Ordering::Relaxed) >> 1) as usize]
}

pub fn is_dark() -> bool {
    CURRENT.load(Ordering::Relaxed) & 1 == 0
}

pub fn colors() -> &'static Palette {
    let theme = current();
    if is_dark() { &theme.dark } else { &theme.light }
}

/// Switch to `name` in `appearance` and repaint every window.
///
/// Light and Dark also force the window chrome to match; Auto hands it back to
/// macOS and reads what it's set to. Classic is dark whatever is asked, and
/// leaves the chrome alone, as it always has.
pub fn apply(name: ThemeName, appearance: Appearance, cx: &mut App) {
    let forced = match appearance {
        _ if !name.has_light() => None,
        Appearance::Auto => None,
        Appearance::Light => Some(WindowAppearance::Light),
        Appearance::Dark => Some(WindowAppearance::Dark),
    };
    cx.set_window_appearance(forced);
    let light = name.has_light()
        && matches!(cx.window_appearance(), WindowAppearance::Light | WindowAppearance::VibrantLight);
    let packed = ((name.theme().name as u8) << 1) | light as u8;
    if CURRENT.swap(packed, Ordering::Relaxed) != packed {
        cx.refresh_windows();
    }
}

/// A corner radius for this theme, from Classic's size in pixels.
pub fn radius(classic: f32) -> Pixels {
    px(classic * current().radius)
}

/// The `i`th hue of the chart's ladder, for the ground it sits on.
pub fn hue(i: usize) -> u32 {
    let ladder = if is_dark() { &HUES_DARK } else { &HUES_LIGHT };
    ladder[i % ladder.len()]
}

/// The text colour that reads on a fill of `color`: the accent's and the
/// danger's own, white on anything else.
pub fn on(color: u32) -> u32 {
    let c = colors();
    if color == c.accent {
        c.on_accent
    } else if color == c.danger {
        c.on_danger
    } else {
        0xffffff
    }
}

/// `color` at `alpha` opacity.
pub fn alpha(color: u32, alpha: f32) -> Hsla {
    let c = crate::sunburst::rgb_to_hsla(color);
    gpui::hsla(c.hue.into_positive_degrees() / 360.0, c.saturation, c.lightness, alpha)
}

/// A see-through wash that lifts what's under it: white on dark grounds,
/// black on light ones. Scrollbars, pills, hovered swatches.
pub fn overlay(alpha: f32) -> Hsla {
    if is_dark() { gpui::hsla(0., 0., 1., alpha) } else { gpui::hsla(0., 0., 0., alpha * 0.8) }
}

/// The text field's caret and selection. gpui's defaults (white, navy) are
/// Classic's and vanish on a light ground.
pub fn input_caret() -> Hsla {
    if is_dark() { gpui::white() } else { alpha(colors().text, 1.0) }
}

pub fn input_selection() -> Hsla {
    if is_dark() { gpui::hsla(210. / 360., 0.519, 0.31, 0.5) } else { alpha(colors().accent, 0.25) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_sit_at_their_own_index() {
        for (ix, theme) in THEMES.iter().enumerate() {
            assert_eq!(theme.name as usize, ix);
        }
    }

    #[test]
    fn mix_ends_and_middle() {
        assert_eq!(mix(0x000000, 0xffffff, 0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 100), 0xffffff);
        assert_eq!(mix(0x204060, 0x406080, 50), 0x305070);
    }

    #[test]
    fn classic_is_the_shipped_look() {
        // Someone who never opens Settings gets the app they had yesterday.
        assert_eq!(ThemeName::default(), ThemeName::Classic);
        let c = &THEMES[0].dark;
        assert_eq!((c.bg, c.panel, c.card, c.accent), (0x1c1d21, 0x232529, 0x2a2c31, 0x4f9dff));
        assert_eq!(THEMES[0].row_height, 30.0);
    }
}
