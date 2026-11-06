//! Toolbar icons drawn as vectors, after SF Symbols' regular weight at a 16 pt
//! optical size: chevrons, an up arrow, the sidebar panes.
//!
//! Drawn, not typed: glyphs like ‹ › ↑ come out thin and off-centre in the
//! UI font, and symbol glyphs go through the emoji path and all but vanish on
//! the glassy toolbar. Every shape is laid out on a 16 × 16 grid and scaled
//! to the element's bounds, so they share one optical size and one stroke.

use super::*;
use gpui::{FillOptions, FillRule, Hsla, PathBuilder, PathStyle};

/// The symbols the toolbar uses, named as in SF Symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Symbol {
    ChevronLeft,
    ChevronRight,
    ArrowUp,
    /// `sidebar.left`; `shown` fills the pane, so the button says whether the
    /// sidebar is there as well as where it would go.
    SidebarLeft { shown: bool },
    /// `sidebar.right`, the cleanup column's mirror of the above.
    SidebarRight { shown: bool },
    /// `doc.on.doc`: Copy Path on a row.
    DocOnDoc,
    /// `magnifyingglass`: Reveal in Finder on a row, as Safari's downloads have it.
    Magnifier,
}

/// SF Symbols' regular weight at 16 pt is about a 1.5 pt line.
const STROKE: f32 = 1.5;
/// The pane outline is a touch lighter, as in the real `sidebar.left`: a
/// rectangle that heavy reads as a button within the button.
const OUTLINE: f32 = 1.25;

/// A 16 pt symbol in `color`. Its size is fixed; the button around it decides
/// the padding and the hit area.
pub(super) fn symbol(symbol: Symbol, color: Hsla) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, _| paint(symbol, color, bounds, window))
        .size(px(16.))
        .flex_none()
}

/// A symbol `size` pixels square in the text colour around it, so a hover
/// that brightens a button's text brightens its icon too.
pub(crate) fn inline_symbol(symbol: Symbol, size: f32) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let color = window.text_style().color;
        paint(symbol, color, bounds, window)
    })
    .size(px(size))
    .flex_none()
}

fn paint(symbol: Symbol, color: Hsla, bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = f32::from(bounds.size.width) / 16.0;
    let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    // Grid units to window pixels.
    let at = move |x: f32, y: f32| point(px(ox + x * scale), px(oy + y * scale));
    match symbol {
        Symbol::ChevronLeft => strokes(&[&[(10.25, 2.75), (5.0, 8.0), (10.25, 13.25)]], STROKE * scale, &at, color, window),
        Symbol::ChevronRight => strokes(&[&[(5.75, 2.75), (11.0, 8.0), (5.75, 13.25)]], STROKE * scale, &at, color, window),
        Symbol::ArrowUp => strokes(
            &[&[(8.0, 13.5), (8.0, 2.75)], &[(3.25, 7.5), (8.0, 2.75), (12.75, 7.5)]],
            STROKE * scale,
            &at,
            color,
            window,
        ),
        Symbol::SidebarLeft { shown } => sidebar(true, shown, &at, scale, color, window),
        Symbol::SidebarRight { shown } => sidebar(false, shown, &at, scale, color, window),
        Symbol::DocOnDoc => {
            // The front page whole, the back one only where it shows: an
            // L along the top and left, as SF Symbols draws it.
            let mut front = PathBuilder::stroke(px(OUTLINE * scale));
            rounded_rect(&mut front, &at, (5.5, 5.0, 13.5, 14.5), [1.75; 4]);
            if let Ok(path) = front.build() {
                window.paint_path(path, color);
            }
            strokes(&[&[(3.0, 10.5), (3.0, 3.75), (3.75, 2.25), (9.5, 2.25), (10.75, 3.0)]], OUTLINE * scale, &at, color, window);
        }
        Symbol::Magnifier => {
            let mut lens = PathBuilder::stroke(px(STROKE * scale));
            let (cx, cy, r) = (6.75, 6.75, 4.5);
            for i in 0..=32 {
                let t = std::f32::consts::TAU * i as f32 / 32.0;
                let p = at(cx + r * t.cos(), cy + r * t.sin());
                if i == 0 { lens.move_to(p) } else { lens.line_to(p) }
            }
            lens.close();
            if let Ok(path) = lens.build() {
                window.paint_path(path, color);
            }
            strokes(&[&[(10.1, 10.1), (13.75, 13.75)]], STROKE * 1.2 * scale, &at, color, window);
        }
    }
}

/// The window outline with its divider, and the pane filled while shown.
fn sidebar(left: bool, shown: bool, at: &dyn Fn(f32, f32) -> Point<Pixels>, scale: f32, color: Hsla, window: &mut Window) {
    let (x0, y0, x1, y1, r) = (1.25, 3.0, 14.75, 13.0, 2.5);
    let divider = if left { 6.0 } else { 10.0 };
    if shown {
        // The pane, inset half a line so the fill doesn't fatten the outline.
        let inset = OUTLINE / 2.0;
        let (px0, px1) = if left { (x0 + inset, divider) } else { (divider, x1 - inset) };
        let radii = if left { [r - inset, 0.0, 0.0, r - inset] } else { [0.0, r - inset, r - inset, 0.0] };
        let mut fill = PathBuilder::fill();
        rounded_rect(&mut fill, at, (px0, y0 + inset, px1, y1 - inset), radii);
        if let Ok(path) = fill.build() {
            let mut pane = color;
            pane.alpha *= 0.45;
            window.paint_path(path, pane);
        }
    }
    let mut outline = PathBuilder::stroke(px(OUTLINE * scale));
    rounded_rect(&mut outline, at, (x0, y0, x1, y1), [r; 4]);
    if let Ok(path) = outline.build() {
        window.paint_path(path, color);
    }
    strokes(&[&[(divider, y0), (divider, y1)]], OUTLINE * scale, at, color, window);
}

/// A closed rounded rectangle, corners in the order top-left, top-right,
/// bottom-right, bottom-left. Arcs are short polylines: at 16 pt nothing
/// finer shows.
fn rounded_rect(b: &mut PathBuilder, at: &dyn Fn(f32, f32) -> Point<Pixels>, (x0, y0, x1, y1): (f32, f32, f32, f32), radii: [f32; 4]) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let corners = [
        (x0 + radii[0], y0 + radii[0], radii[0], PI),
        (x1 - radii[1], y0 + radii[1], radii[1], PI + FRAC_PI_2),
        (x1 - radii[2], y1 - radii[2], radii[2], 0.0),
        (x0 + radii[3], y1 - radii[3], radii[3], FRAC_PI_2),
    ];
    let mut first = true;
    for (cx, cy, r, start) in corners {
        for i in 0..=6 {
            let t = start + FRAC_PI_2 * i as f32 / 6.0;
            let p = at(cx + r * t.cos(), cy + r * t.sin());
            if first {
                b.move_to(p);
                first = false;
            } else {
                b.line_to(p);
            }
        }
    }
    b.close();
}

/// Polylines with round caps and joins, as SF Symbols draws them. gpui's
/// stroker has neither, so each segment is a filled capsule, and all of them
/// go into one non-zero path: where two meet, the overlap is one surface,
/// not a darker spot when the icon is dimmed.
fn strokes(lines: &[&[(f32, f32)]], width: f32, at: &dyn Fn(f32, f32) -> Point<Pixels>, color: Hsla, window: &mut Window) {
    use std::f32::consts::PI;
    let base = at(0.0, 0.0);
    let unit = f32::from(at(1.0, 0.0).x) - f32::from(base.x);
    let r = width / 2.0 / unit.max(f32::EPSILON);
    let mut b = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::default().with_fill_rule(FillRule::NonZero)));
    for line in lines {
        for pair in line.windows(2) {
            let ((ax, ay), (bx, by)) = (pair[0], pair[1]);
            let angle = (by - ay).atan2(bx - ax);
            // Down one side, round the far end, back up the other, round the
            // near end: the same turn for every segment, so non-zero unions them.
            const STEPS: usize = 8;
            let normal = angle - PI / 2.0;
            b.move_to(at(ax + r * normal.cos(), ay + r * normal.sin()));
            for i in 0..=STEPS {
                let t = normal + PI * i as f32 / STEPS as f32;
                b.line_to(at(bx + r * t.cos(), by + r * t.sin()));
            }
            for i in 0..=STEPS {
                let t = normal + PI + PI * i as f32 / STEPS as f32;
                b.line_to(at(ax + r * t.cos(), ay + r * t.sin()));
            }
            b.close();
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}
