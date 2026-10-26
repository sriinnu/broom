//! The investigator: a small ligne-claire figure in a flat cap, trench coat and
//! scarf who paces the strip with a magnifying glass while a scan runs, with a
//! speech bubble saying where it is looking. Flat colour, one contour weight,
//! no gradients — the same vocabulary as the icon concepts. Drawn with paths,
//! so it works at any size and needs no image assets.

use std::time::Instant;

use gpui::{Context, Hsla, PathBuilder, Pixels, Point, SharedString, Window, canvas, div, point, prelude::*, px, rgb};

use crate::clock;
use crate::sunburst::Geometry;

const INK: u32 = 0x1d1a17;
const CREAM: u32 = 0xf3ead8;
const LENS: u32 = 0xbfe3f2;
const LENS_SHINE: u32 = 0xffffff;
const HANDLE: u32 = 0xb5752e;
const DUST: u32 = 0x9a9ea6;
const COAT: u32 = 0xc9a46a;
const CAP: u32 = 0x5a6b7a;
const SCARF: u32 = 0xd85164;
const SKIN: u32 = 0xf4cfa8;

/// What the bubble says, cycling as time passes, with the folder being read.
const VERBS: &[&str] = &[
    "Rummaging through",
    "Peeking into",
    "Sniffing around",
    "Counting everything in",
    "Poking at",
    "Turning over",
    "Having a look at",
];

fn started() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(clock::now)
}

/// Seconds since the first frame, on the app clock (pauses with a recording).
fn t() -> f32 {
    clock::since(started()).as_secs_f32()
}

/// The line for the bubble: a verb that changes every couple of seconds, the
/// folder being read, and a reaction once the count gets big.
pub fn caption(current: &str, files: u64) -> Caption {
    let verb = VERBS[((t() / 2.3) as usize) % VERBS.len()];
    let place = if current.is_empty() { "the first folders".to_string() } else { current.to_string() };
    let aside: SharedString = match files {
        0..=50_000 => "".into(),
        50_001..=500_000 => format!("{} files so far, nothing to worry about.", crate::scan::format_count(files)).into(),
        500_001..=2_000_000 => format!("Whoa, {} files already.", crate::scan::format_count(files)).into(),
        _ => format!("{} files. This is a big one.", crate::scan::format_count(files)).into(),
    };
    Caption { line: format!("{verb}…").into(), place: place.into(), aside }
}

/// Before any scan: waiting, not rummaging.
pub fn idle() -> Caption {
    const LINES: &[(&str, &str)] = &[
        ("Ready when you are.", "pick a disk or a folder on the left"),
        ("Magnifier polished.", "nothing scanned yet"),
        ("Nothing to sweep yet.", "a scan takes seconds, a whole disk under a minute"),
    ];
    let (line, place) = LINES[((t() / 4.0) as usize) % LINES.len()];
    Caption { line: line.into(), place: place.into(), aside: "".into() }
}

/// The bubble's three lines: the verb, the folder (shown with its start
/// trimmed), and a reaction that may be empty.
pub struct Caption {
    pub line: SharedString,
    pub place: SharedString,
    pub aside: SharedString,
}

fn to_hsla(color: u32) -> Hsla {
    use palette::IntoColor;
    rgb(color).into_color()
}

fn disc(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    Geometry::ring(center, 0.0, radius).paint_disc(window, radius, color);
}

/// Upper half of a disc: the crown of the cap.
fn dome(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    Geometry::ring(center, 0.0, radius).paint_sector(window, 1, 0.75, 1.25, color);
}

fn stroke(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, width: f32, color: Hsla) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A stroke with round ends (lyon's default cap is butt).
fn rod(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, width: f32, color: Hsla) {
    stroke(window, from, to, width, color);
    disc(window, from, width / 2.0, color);
    disc(window, to, width / 2.0, color);
}

/// A contoured rod: ink slightly wider underneath, the colour on top. `width`
/// is in figure units, scaled by `s`.
fn limb(window: &mut Window, s: f32, from: Point<Pixels>, to: Point<Pixels>, width: f32, color: Hsla, ink: Hsla) {
    rod(window, from, to, (width + 2.0 * CONTOUR) * s, ink);
    rod(window, from, to, width * s, color);
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

/// Contour weight around every fill, in figure units (the figure is drawn in a
/// 64-unit-tall box and scaled to the strip's height).
const CONTOUR: f32 = 1.6;
/// How long each pose is held, and the fraction of it spent easing in.
const POSE_SECS: f32 = 2.4;
const EASE: f32 = 0.2;

#[derive(Clone, Copy)]
struct Pose {
    /// -1 looks left, +1 looks right; eyes, brim, lens and lean all follow it.
    face: f32,
    /// 0 upright, 1 bent over looking at the floor.
    bend: f32,
}

const POSES: [Pose; 3] = [Pose { face: -1.0, bend: 0.0 }, Pose { face: 1.0, bend: 0.0 }, Pose { face: 0.9, bend: 1.0 }];

/// Where the figure is in its cycle: the pose (eased from the previous one),
/// its position across the strip in -1..1, and its signed speed in -1..1.
/// Looking left it walks left, looking right it walks right, and it stands
/// still while studying the floor.
fn frame(t: f32) -> (Pose, f32, f32) {
    let u = t / POSE_SECS;
    let idx = (u as usize) % POSES.len();
    let p = u.fract();
    let (prev, cur) = (POSES[(idx + POSES.len() - 1) % POSES.len()], POSES[idx]);
    let k = smooth(p / EASE);
    let pose = Pose { face: lerp(prev.face, cur.face, k), bend: lerp(prev.bend, cur.bend, k) };
    let (x, speed) = match idx {
        0 => (lerp(1.0, -1.0, smooth(p)), -4.0 * p * (1.0 - p)),
        1 => (lerp(-1.0, 1.0, smooth(p)), 4.0 * p * (1.0 - p)),
        _ => (1.0, 0.0),
    };
    (pose, x, speed)
}

/// The figure's frame: origin at the top centre of its 64-unit box, scaled.
struct Fig {
    ox: f32,
    oy: f32,
    s: f32,
}

impl Fig {
    fn at(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(self.ox + x * self.s), px(self.oy + y * self.s))
    }
}

/// The trench coat: a trapezoid with rounded corners, inflated by `e` for the
/// contour pass.
fn coat(window: &mut Window, fig: &Fig, bob: f32, lean: f32, e: f32, color: Hsla) {
    let (top, bottom, a, c, k) = (31.0 - e + bob, 50.0 + e + bob, 8.5 + e, 12.5 + e, 3.0);
    let mut b = PathBuilder::fill();
    b.move_to(fig.at(-a + k + lean, top));
    b.line_to(fig.at(a - k + lean, top));
    b.curve_to(fig.at(a + lean, top + k), fig.at(a + lean, top));
    b.line_to(fig.at(c, bottom - k));
    b.curve_to(fig.at(c - k, bottom), fig.at(c, bottom));
    b.line_to(fig.at(-c + k, bottom));
    b.curve_to(fig.at(-c, bottom - k), fig.at(-c, bottom));
    b.line_to(fig.at(-a + lean, top + k));
    b.curve_to(fig.at(-a + k + lean, top), fig.at(-a + lean, top));
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// Paint the investigator — a small round-headed figure in a flat cap, trench
/// coat and scarf, holding the magnifier out — at its current position inside
/// `bounds`. `done` is the scan's fraction when known: the lens rim fills
/// clockwise with it.
pub fn paint_investigator(window: &mut Window, bounds: gpui::Bounds<Pixels>, done: Option<f32>) {
    let t = t();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    let s = h / 64.0;
    let (Pose { face, bend }, x, speed) = frame(t);
    let ink = to_hsla(INK);
    let coat_color = to_hsla(COAT);
    let skin = to_hsla(SKIN);
    let cap = to_hsla(CAP);
    let scarf = to_hsla(SCARF);
    let c = CONTOUR;

    // Lens radius and reach, in figure units; the walk amplitude keeps the
    // outstretched magnifier inside the strip.
    let r = 10.5;
    let reach = 31.0;
    let amp = (w / 2.0 / s - reach - r - 2.0).max(0.0);
    let fig = Fig { ox: f32::from(bounds.origin.x) + w / 2.0 + x * amp * s, oy: f32::from(bounds.origin.y), s };
    let bob = (t * 3.1).cos() * 1.2 + bend * 2.0;
    // Bending over, the shoulders shift the way the figure faces.
    let lean = bend * face * 5.0;
    let stride = (t * 11.0).sin() * 3.0 * speed.abs();

    // Motion lines trail the direction of travel, at coat height, and fade
    // with the speed.
    let dir = if speed < 0.0 { -1.0 } else { 1.0 };
    let pace = speed.abs();
    if pace > 0.05 {
        for i in 0..3 {
            let phase = (t * 4.0 + i as f32 * 1.1) % 1.0;
            let len = 6.0 + 8.0 * (1.0 - phase);
            let x0 = -dir * (14.0 + i as f32 * 3.0 + phase * 6.0);
            let y = 38.0 + (i as f32 - 1.0) * 6.0;
            let mut color = ink;
            color.alpha = 0.55 * (1.0 - phase) * pace;
            stroke(window, fig.at(x0, y), fig.at(x0 - dir * len, y), 2.0 * s, color);
        }
        // Dust puffs kicked up behind the feet.
        for i in 0..3 {
            let phase = (t * 1.7 + i as f32 * 0.37) % 1.0;
            let x = -dir * (6.0 + phase * 22.0);
            let y = 58.0 - phase * 10.0 + (i as f32) * 2.0;
            let mut color = to_hsla(DUST);
            color.alpha = 0.6 * (1.0 - phase) * pace;
            disc(window, fig.at(x, y), (1.5 + 2.5 * phase) * s, color);
        }
    }

    // Legs, then shoes, planted on the floor; they swing with the stride.
    for side in [-1.0, 1.0] {
        let hip = fig.at(side * 4.0, 47.0 + bob);
        let foot = fig.at(side * 4.5 + side * stride + face * 1.5, 60.0);
        rod(window, hip, foot, 4.2 * s, ink);
        disc(window, foot, 2.8 * s, ink);
    }
    // The back arm hangs at the side, behind the coat.
    limb(window, s, fig.at(-face * 7.0 + lean, 34.0 + bob), fig.at(-face * 11.5, 46.0 + bob), 3.8, coat_color, ink);
    // Coat: contour, fill, belt.
    coat(window, &fig, bob, lean, c, ink);
    coat(window, &fig, bob, lean, 0.0, coat_color);
    stroke(window, fig.at(-10.0, 42.0 + bob), fig.at(10.0, 42.0 + bob), 1.6 * s, ink);
    // Scarf: a tail that flutters behind, and the band around the neck.
    let flutter = (t * 6.0).sin() * 2.0 * (0.3 + pace);
    limb(window, s, fig.at(-face * 5.0 + lean, 31.0 + bob), fig.at(-face * 9.0 + flutter, 42.0 + bob), 3.6, scarf, ink);
    limb(window, s, fig.at(-7.0 + lean, 30.0 + bob), fig.at(7.0 + lean, 30.0 + bob), 4.2, scarf, ink);

    // Head: leans into the look and drops when bending.
    let hr = 9.0;
    let (hx, hy) = (face * 2.0 + lean, 21.0 + bend * 5.0 + bob);
    disc(window, fig.at(hx, hy), (hr + c) * s, ink);
    disc(window, fig.at(hx, hy), hr * s, skin);
    // Flat cap: a dome over the crown, and one stroke that is both the band
    // and the brim, reaching out the way the figure is looking.
    let crown = hy - 2.0;
    dome(window, fig.at(hx, crown), (hr + 0.8 + c) * s, ink);
    dome(window, fig.at(hx, crown), (hr + 0.8) * s, cap);
    limb(window, s, fig.at(hx - face * 9.0, crown), fig.at(hx + face * 15.0, crown + 0.6), 1.9, cap, ink);
    // Eyes: two dots that look where the magnifier points; a short line when
    // blinking.
    let blink = (t + 1.3) % 3.7 < 0.12 || (t + 0.4) % 6.1 < 0.12;
    let ey = hy + 3.0 + bend * 1.5;
    for side in [-1.0, 1.0] {
        let ex = hx + face * 3.8 + side * 2.9;
        if blink {
            rod(window, fig.at(ex - 1.7, ey), fig.at(ex + 1.7, ey), 1.4 * s, ink);
        } else {
            disc(window, fig.at(ex, ey), 1.9 * s, ink);
        }
    }

    // The magnifier: lens out in front at eye level, or down near the floor;
    // the handle points back to the hand, the arm reaches from the shoulder.
    let (lx, ly) = (face * reach, 24.0 + bend * 26.0 + bob);
    let (sx, sy) = (face * 4.0 + lean, 34.0 + bend * 4.0 + bob);
    let (dx, dy) = (sx - lx, sy - ly);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let grip = r * lerp(1.6, 1.1, bend);
    let (gx, gy) = (lx + ux * grip, ly + uy * grip);
    limb(window, s, fig.at(sx, sy), fig.at(gx, gy), 3.8, coat_color, ink);
    limb(window, s, fig.at(lx + ux * r * 0.85, ly + uy * r * 0.85), fig.at(gx, gy), 3.8, to_hsla(HANDLE), ink);
    disc(window, fig.at(gx, gy), (2.6 + c) * s, ink);
    disc(window, fig.at(gx, gy), 2.6 * s, skin);

    // Lens: contour ring, glass, a shine.
    let center = fig.at(lx, ly);
    let (r, rim) = (r * s, (r + c + 0.4) * s);
    disc(window, center, rim, ink);
    disc(window, center, r, to_hsla(LENS));
    let mut shine = to_hsla(LENS_SHINE);
    shine.alpha = 0.85;
    Geometry::ring(center, r * 0.55, r * 0.8).paint_sector(window, 1, 0.83, 0.97, shine);
    // The rim as a progress ring, when there is a total to measure against.
    if let Some(done) = done {
        let mut track = to_hsla(CREAM);
        track.alpha = 0.35;
        Geometry::ring(center, r + 0.5, rim).paint_sector(window, 1, 0.0, 1.0, track);
        Geometry::ring(center, r + 0.5, rim).paint_sector(window, 1, 0.0, done.clamp(0.0, 1.0), to_hsla(CREAM));
    }
    window.request_animation_frame();
}

/// The whole strip: investigator on the left, a speech bubble with the caption
/// on the right. `done` is the scan's fraction if known.
pub fn investigator(caption: Caption, done: Option<f32>, height: f32) -> impl IntoElement {
    let Caption { line, place, aside } = caption;
    div()
        .flex()
        .items_center()
        .gap_1()
        .w_full()
        .h(px(height))
        .child(
            canvas(|_, _, _| {}, move |bounds, _, window, _| paint_investigator(window, bounds, done))
                .w(px(height * 2.1))
                .h(px(height))
                .flex_none(),
        )
        // The bubble's tail: ink triangle with a cream one inset.
        .child(
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    let x1: f32 = bounds.origin.x.into();
                    let y0: f32 = bounds.origin.y.into();
                    let w: f32 = bounds.size.width.into();
                    let h: f32 = bounds.size.height.into();
                    let tri = |inset: f32, color: Hsla, window: &mut Window| {
                        let mut b = PathBuilder::fill();
                        b.move_to(point(px(x1 + inset), px(y0 + h / 2.0)));
                        b.line_to(point(px(x1 + w + 2.0), px(y0 + h * 0.2 + inset)));
                        b.line_to(point(px(x1 + w + 2.0), px(y0 + h * 0.8 - inset)));
                        b.close();
                        if let Ok(path) = b.build() {
                            window.paint_path(path, color);
                        }
                    };
                    tri(0.0, to_hsla(INK), window);
                    tri(2.6, to_hsla(CREAM), window);
                },
            )
            .w(px(10.))
            .h(px(18.))
            .flex_none(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px_3()
                .py_1p5()
                .rounded_lg()
                .bg(rgb(CREAM))
                .border_2()
                .border_color(rgb(INK))
                .text_color(rgb(INK))
                .flex()
                .flex_col()
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(line))
                .child(div().text_xs().text_ellipsis_start().overflow_hidden().whitespace_nowrap().opacity(0.75).child(place))
                .when(!aside.is_empty(), |d| d.child(div().text_xs().opacity(0.75).child(aside))),
        )
}

/// Tintin's narration yellow, for the caption box.
const CAPTION: u32 = 0xf7e7a3;

/// A full path in a ligne-claire caption box ("Meanwhile, in…"), with a little
/// balloon tail pointing up at the pointer. The tooltip for any path that's cut
/// short where it sits.
pub struct PathCaption(pub SharedString);

impl Render for PathCaption {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Paths have no spaces to wrap at; a zero-width space after each `/`
        // lets a long one break on folder boundaries instead of overflowing.
        let wrapped = self.0.replace('/', "/\u{200B}");
        div()
            .relative()
            .pt(px(9.))
            .child(
                div()
                    .max_w(px(460.))
                    .px_2p5()
                    .py_1p5()
                    .rounded(px(3.))
                    .bg(rgb(CAPTION))
                    .border_2()
                    .border_color(rgb(INK))
                    .shadow_md()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .text_color(rgb(INK))
                    .child(div().text_size(px(9.)).font_weight(gpui::FontWeight::BOLD).child("MEANWHILE, IN…"))
                    .child(div().text_xs().font_family(crate::theme::current().mono).child(wrapped)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                        // Ink first, then the yellow a contour inside it, running
                        // down over the box's top border so the tail opens into it.
                        let mut ink = PathBuilder::fill();
                        ink.move_to(at(0.0, 11.0));
                        ink.line_to(at(5.0, 0.0));
                        ink.line_to(at(16.0, 11.0));
                        ink.close();
                        if let Ok(path) = ink.build() {
                            window.paint_path(path, to_hsla(INK));
                        }
                        let mut fill = PathBuilder::fill();
                        fill.move_to(at(2.6, 12.5));
                        fill.line_to(at(5.6, 4.2));
                        fill.line_to(at(12.6, 12.5));
                        fill.close();
                        if let Ok(path) = fill.build() {
                            window.paint_path(path, to_hsla(CAPTION));
                        }
                    },
                )
                .absolute()
                .top_0()
                .left(px(14.))
                .w(px(16.))
                .h(px(13.)),
            )
    }
}
