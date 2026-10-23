//! The drop: when a move to the Trash is confirmed, the ticked folders and files
//! tumble one by one into a ligne-claire bin while the real move runs. The lid
//! opens for the first item, stays open until the last one is in and the move
//! has really finished, then closes with a bounce. It never blocks the move and
//! never runs longer than about a second and a half, however long the list.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, canvas, point, prelude::*, px, rgb};

use crate::clock;
use crate::scan::Kind;

/// Move `paths` to the Trash. Finder does it first, so "Put Back" works and
/// the familiar sound plays; whatever Finder won't take (it answers -10010,
/// "can't handle objects of this class", for some big folders such as a
/// node_modules with ten thousand files) goes through NSFileManager instead.
/// Not atomic: on an error, the caller checks which paths are still there.
pub fn send(paths: &[PathBuf]) -> Result<(), String> {
    use trash::TrashContext;
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    if paths.is_empty() {
        return Ok(());
    }
    let finder = trash::delete_all(paths);
    let left: Vec<&Path> = paths.iter().map(PathBuf::as_path).filter(|p| p.symlink_metadata().is_ok()).collect();
    if left.is_empty() {
        return Ok(());
    }
    let mut context = TrashContext::default();
    context.set_delete_method(DeleteMethod::NsFileManager);
    let mut errors = Vec::new();
    for path in left {
        if let Err(error) = context.delete(path) {
            errors.push(format!("{}: {error}", path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let why = finder.err().map(|e| format!(" (Finder said: {e})")).unwrap_or_default();
        Err(format!("{}{why}", errors.join("; ")))
    }
}

const INK: u32 = 0x1d1a17;
const CREAM: u32 = 0xf3ead8;
const CAN: u32 = 0x5a6b7a;
const CAN_LIGHT: u32 = 0x7d8e9c;
const CAN_DARK: u32 = 0x3d4a54;
const FOLDER: u32 = 0xe9c46a;
const FOLDER_TAB: u32 = 0xd4a84b;
const PAPER_FOLD: u32 = 0xd9cfbd;
const CONTOUR: f32 = 1.5;

/// Seconds: the lid swings open, the first item leaves, each flight, the lid
/// closes, and the bounce after.
const LID_OPEN: f32 = 0.18;
const FIRST: f32 = 0.12;
const FLIGHT: f32 = 0.42;
const CLOSE: f32 = 0.22;
const BOUNCE: f32 = 0.36;
/// Lanes across the top of the stage that items wait in.
const SLOTS: usize = 10;

pub struct Item {
    pub kind: Kind,
    /// 0.6..1: bigger things draw bigger.
    pub weight: f32,
}

impl Item {
    /// One item per node; the weight follows the log of its size against the largest.
    pub fn from_sizes(items: &[(Kind, u64)]) -> Vec<Item> {
        let max = items.iter().map(|&(_, s)| s).max().unwrap_or(1).max(1) as f32;
        items
            .iter()
            .map(|&(kind, size)| {
                let rel = ((size.max(1) as f32).ln() / max.ln()).clamp(0.0, 1.0);
                Item { kind, weight: 0.6 + 0.4 * rel }
            })
            .collect()
    }
}

pub struct Drop {
    start: Instant,
    items: Vec<Item>,
    stagger: f32,
    bytes: u64,
    /// Seconds after `start` when the real move finished; the lid can't close before.
    settled: Cell<Option<f32>>,
}

impl Drop {
    pub fn new(items: Vec<Item>, bytes: u64) -> Rc<Self> {
        let n = items.len().max(1);
        let stagger = (0.7 / n as f32).clamp(0.012, 0.1);
        Rc::new(Drop { start: clock::now(), items, stagger, bytes, settled: Cell::new(None) })
    }

    fn t(&self) -> f32 {
        clock::since(self.start).as_secs_f32()
    }

    fn launch(&self, i: usize) -> f32 {
        FIRST + i as f32 * self.stagger
    }

    fn last_land(&self) -> f32 {
        self.launch(self.items.len().saturating_sub(1)) + FLIGHT
    }

    /// The move really finished: the lid may close once the last item is in.
    pub fn settle(&self) {
        if self.settled.get().is_none() {
            self.settled.set(Some(self.t()));
        }
    }

    fn close_at(&self) -> Option<f32> {
        self.settled.get().map(|s| s.max(self.last_land() + 0.1))
    }

    /// After `settle`: how long until the lid is shut and the bounce is over.
    pub fn remaining(&self) -> Duration {
        match self.close_at() {
            Some(close) => Duration::from_secs_f32((close + CLOSE + BOUNCE - self.t()).max(0.0)),
            None => Duration::ZERO,
        }
    }

    fn finished(&self) -> bool {
        self.close_at().is_some_and(|close| self.t() >= close + CLOSE + BOUNCE)
    }

    /// Items in the bin so far, and the bytes they stand for.
    pub fn landed(&self) -> (usize, u64) {
        let t = self.t();
        let n = (0..self.items.len()).filter(|&i| t >= self.launch(i) + FLIGHT).count();
        let fraction = if self.items.is_empty() { 1.0 } else { n as f64 / self.items.len() as f64 };
        (n, (self.bytes as f64 * fraction) as u64)
    }

    pub fn total(&self) -> usize {
        self.items.len()
    }
}

/// The stage, filling its container: the bin at the bottom, the queue along the top.
pub fn stage(drop: Rc<Drop>) -> impl IntoElement {
    canvas(|_, _, _| {}, move |bounds, _, window, _| paint(window, bounds, &drop)).size_full()
}

fn to_hsla(color: u32) -> Hsla {
    use palette::IntoColor;
    rgb(color).into_color()
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

fn fill(window: &mut Window, points: &[Point<Pixels>], color: Hsla) {
    let mut builder = PathBuilder::fill();
    builder.add_polygon(points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn outline(window: &mut Window, points: &[Point<Pixels>], width: f32, color: Hsla) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.add_polygon(points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, width: f32, color: Hsla) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn disc(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    crate::sunburst::Geometry::ring(center, 0.0, radius).paint_disc(window, radius, color);
}

/// A shape given in local units, placed at `at`, scaled and rotated.
fn place(shape: &[(f32, f32)], at: (f32, f32), scale: f32, angle: f32) -> Vec<Point<Pixels>> {
    let (sin, cos) = angle.sin_cos();
    shape
        .iter()
        .map(|&(x, y)| {
            let (x, y) = (x * scale, y * scale);
            point(px(at.0 + x * cos - y * sin), px(at.1 + x * sin + y * cos))
        })
        .collect()
}

/// A filled, contoured shape.
fn piece(window: &mut Window, shape: &[(f32, f32)], at: (f32, f32), scale: f32, angle: f32, color: Hsla, ink: Hsla) {
    let points = place(shape, at, scale, angle);
    fill(window, &points, color);
    outline(window, &points, CONTOUR * scale.max(0.6), ink);
}

/// An ellipse as a polygon: (cx, cy), radii, and which part (0..1 of the turn, from the left).
fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32, from: f32, to: f32) -> Vec<Point<Pixels>> {
    let steps = 28;
    (0..=steps)
        .map(|i| {
            let a = std::f32::consts::TAU * lerp(from, to, i as f32 / steps as f32);
            point(px(cx + rx * a.cos()), px(cy + ry * a.sin()))
        })
        .collect()
}

const FOLDER_BODY: [(f32, f32); 4] = [(-11.0, -6.0), (11.0, -6.0), (11.0, 8.0), (-11.0, 8.0)];
const FOLDER_TAB_SHAPE: [(f32, f32); 4] = [(-11.0, -6.0), (-3.0, -6.0), (-1.0, -9.5), (-11.0, -9.5)];
const PAPER: [(f32, f32); 5] = [(-7.0, -9.5), (3.0, -9.5), (7.0, -5.5), (7.0, 9.5), (-7.0, 9.5)];
const PAPER_EAR: [(f32, f32); 3] = [(3.0, -9.5), (7.0, -5.5), (3.0, -5.5)];

fn icon(window: &mut Window, kind: Kind, at: (f32, f32), scale: f32, angle: f32, ink: Hsla) {
    match kind {
        Kind::File => {
            piece(window, &PAPER, at, scale, angle, to_hsla(CREAM), ink);
            let ear = place(&PAPER_EAR, at, scale, angle);
            fill(window, &ear, to_hsla(PAPER_FOLD));
            // Two lines of "text".
            for (i, y) in [(-1.5f32), 2.5].iter().enumerate() {
                let w = if i == 0 { 9.0 } else { 6.0 };
                let a = place(&[(-4.5, *y), (-4.5 + w, *y)], at, scale, angle);
                line(window, a[0], a[1], 1.2 * scale, to_hsla(PAPER_FOLD));
            }
        }
        _ => {
            piece(window, &FOLDER_TAB_SHAPE, at, scale, angle, to_hsla(FOLDER_TAB), ink);
            piece(window, &FOLDER_BODY, at, scale, angle, to_hsla(FOLDER), ink);
        }
    }
}

fn paint(window: &mut Window, bounds: Bounds<Pixels>, drop: &Drop) {
    let t = drop.t();
    let ox: f32 = bounds.origin.x.into();
    let oy: f32 = bounds.origin.y.into();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    let ink = to_hsla(INK);
    // The bin is drawn in a 100-unit-tall box and scaled to the stage.
    let s = (h / 150.0).clamp(0.6, 1.6);
    let cx = ox + w / 2.0;

    // Lid: open quickly, hold, close once the move is really done; then a bounce.
    let close = drop.close_at();
    let mut open = smooth(t / LID_OPEN);
    let mut squash = 0.0;
    if let Some(close) = close {
        open *= 1.0 - smooth((t - close) / CLOSE);
        let u = ((t - close - CLOSE) / BOUNCE).clamp(0.0, 1.0);
        if u > 0.0 && u < 1.0 {
            squash = 0.09 * (std::f32::consts::PI * u).sin() * (1.0 - u);
        }
    }
    let floor = oy + h - 8.0 * s;
    let body_h = 62.0 * s * (1.0 - squash);
    let top_y = floor - body_h;
    let (rx_top, rx_bottom) = (30.0 * s * (1.0 + squash * 0.8), 25.0 * s * (1.0 + squash * 0.8));
    let ry = 6.5 * s;
    let mouth_y = top_y;

    // Body: a trapezoid, three ribs, a highlight, then the inside of the mouth.
    let body = [
        point(px(cx - rx_top), px(top_y)),
        point(px(cx + rx_top), px(top_y)),
        point(px(cx + rx_bottom), px(floor)),
        point(px(cx - rx_bottom), px(floor)),
    ];
    fill(window, &body, to_hsla(CAN));
    for k in [0.3f32, 0.55, 0.8] {
        let y = lerp(top_y, floor, k);
        let rx = lerp(rx_top, rx_bottom, k) - 1.5 * s;
        line(window, point(px(cx - rx), px(y)), point(px(cx + rx), px(y)), 1.4 * s, to_hsla(CAN_DARK));
    }
    line(window, point(px(cx - rx_top + 6.0 * s), px(top_y + 8.0 * s)), point(px(cx - rx_bottom + 5.0 * s), px(floor - 6.0 * s)), 2.2 * s, to_hsla(CAN_LIGHT));
    outline(window, &body, CONTOUR * s, ink);
    // The bottom rim and the back half of the mouth (the hole).
    let bottom = ellipse(cx, floor, rx_bottom, ry * 0.8, 0.0, 0.5);
    fill(window, &bottom, to_hsla(CAN));
    outline(window, &bottom, CONTOUR * s, ink);
    let hole = ellipse(cx, mouth_y, rx_top, ry, 0.0, 1.0);
    fill(window, &hole, to_hsla(CAN_DARK));

    // The queue along the top, and the items in flight.
    let shelf_y = oy + 16.0 * s;
    let lane = |i: usize| ox + w * (0.08 + 0.84 * ((i % SLOTS) as f32 + 0.5) / SLOTS as f32);
    let n = drop.items.len();
    for (i, item) in drop.items.iter().enumerate() {
        let k = (t - drop.launch(i)) / FLIGHT;
        let scale = s * 0.9 * item.weight;
        if k < 0.0 {
            // Waiting: only the next few lanes' worth, so a long list doesn't pile up.
            if i >= SLOTS * 2 {
                continue;
            }
            let stack = (i / SLOTS) as f32;
            let wobble = (t * 3.0 + i as f32).sin() * 0.03;
            icon(window, item.kind, (lane(i), shelf_y - stack * 6.0 * s), scale, wobble, ink);
        } else if k < 0.96 {
            let x0 = lane(i);
            let x1 = cx + ((i % 5) as f32 - 2.0) * 3.0 * s;
            let x = lerp(x0, x1, smooth(k));
            let hop = 18.0 * s;
            let y = shelf_y + (mouth_y - shelf_y) * k * k - hop * (std::f32::consts::PI * k).sin();
            let spin = (0.9 + (i % 3) as f32 * 0.5) * if i % 2 == 0 { 1.0 } else { -1.0 };
            let shrink = if k > 0.72 { lerp(1.0, 0.5, (k - 0.72) / 0.24) } else { 1.0 };
            icon(window, item.kind, (x, y), scale * shrink, k * spin, ink);
        } else {
            // Landed: a puff of dust at the mouth for a moment.
            let since = t - (drop.launch(i) + FLIGHT);
            if since < 0.28 {
                let u = since / 0.28;
                let mut puff = to_hsla(CREAM);
                puff.alpha = 0.7 * (1.0 - u);
                for j in 0..3 {
                    let dx = ((j as f32) - 1.0) * (6.0 + 8.0 * u) * s;
                    let r = (2.8 - 1.6 * u) * s;
                    disc(window, point(px(cx + dx), px(mouth_y - (4.0 + 10.0 * u) * s)), r, puff);
                }
            }
        }
    }

    // The front rim of the mouth, over whatever is falling in.
    let lip = ellipse(cx, mouth_y, rx_top, ry, 0.0, 0.5);
    fill(window, &lip, to_hsla(CAN));
    outline(window, &lip, CONTOUR * s, ink);
    let rim = ellipse(cx, mouth_y, rx_top, ry, 0.0, 1.0);
    outline(window, &rim, CONTOUR * s, ink);

    // The lid, hinged at the back-left of the rim, with a knob on top.
    let hinge = (cx - rx_top - 1.0 * s, mouth_y - 1.5 * s);
    let angle = -1.3 * open;
    let span = 2.0 * rx_top / s + 2.0;
    let lid: [(f32, f32); 6] = [(0.0, 0.0), (span, 0.0), (span - 2.0, -6.0), (span * 0.8, -8.5), (span * 0.2, -8.5), (2.0, -6.0)];
    piece(window, &lid, hinge, s, angle, to_hsla(CAN_LIGHT), ink);
    // The lid's rim: a darker band along its underside.
    let band = place(&[(2.5, -2.2), (span - 2.5, -2.2)], hinge, s, angle);
    line(window, band[0], band[1], 1.4 * s, to_hsla(CAN));
    let knob = place(&[(span / 2.0, -11.0)], hinge, s, angle);
    disc(window, knob[0], (3.2 + CONTOUR) * s, ink);
    disc(window, knob[0], 3.2 * s, to_hsla(CAN_LIGHT));

    let _ = n;
    if !drop.finished() {
        window.request_animation_frame();
    }
}
