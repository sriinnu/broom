//! Sunburst layout, painting and hit testing.
//!
//! Angles are measured in turns (0..1), clockwise from 12 o'clock.

use std::f32::consts::TAU;

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, hsla, point, px};

use crate::scan::{Kind, Tree};

pub const MAX_DEPTH: usize = 6;
/// Children thinner than this are merged into a single "smaller objects" segment.
const MIN_TURNS: f32 = 0.004;
/// Inner rings get more room: the first two carry labels.
const RING_FALLOFF: f32 = 0.78;
const GAP_PX: f32 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Node(usize),
    /// Aggregated tiny children of `parent`.
    Small { parent: usize },
}

/// The categorical ladder the icon, the rail and the chart share: rose, lime,
/// periwinkle, gold, teal, tangerine. Validated to stay apart for every kind
/// of colour vision and to carry dark text.
pub const HUES: [u32; 6] = [0xd85164, 0x699f08, 0x5a76e4, 0xbc8900, 0x009c9c, 0xd26c28];

/// Children of the focused folder past the sixth share one neutral grey.
pub const NO_GROUP: usize = usize::MAX;

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub target: Target,
    /// 1-based ring index.
    pub depth: usize,
    pub start: f32,
    pub end: f32,
    pub kind: Kind,
    /// Which child of the focused folder this slice belongs to, by size rank
    /// (0 = biggest). Colour follows it, so zooming in recolours the children
    /// apart. `NO_GROUP` for the tail past the six hues and for merged slivers.
    pub group: usize,
}

pub fn layout(tree: &Tree, focus: usize) -> Vec<Segment> {
    let mut out = Vec::new();
    layout_children(tree, focus, 1, 0.0, 1.0, NO_GROUP, &mut out);
    out
}

fn layout_children(tree: &Tree, ix: usize, depth: usize, start: f32, end: f32, group: usize, out: &mut Vec<Segment>) {
    let node = &tree.nodes[ix];
    if node.size == 0 {
        return;
    }
    let span = end - start;
    let total = node.size as f64;
    let mut angle = start;
    // Thin children are merged into one "smaller objects" sliver. Children are sorted
    // largest first, except slices pinned to the end (e.g. "Not scanned yet"), so keep
    // going after the thin ones: anything large after them still gets its own segment.
    let mut small: Option<(f32, f32)> = None;
    let flush = |small: &mut Option<(f32, f32)>, out: &mut Vec<Segment>| {
        if let Some((from, width)) = small.take() {
            if width >= MIN_TURNS / 2.0 {
                out.push(Segment { target: Target::Small { parent: ix }, depth, start: from, end: from + width, kind: Kind::File, group: NO_GROUP });
            }
        }
    };
    // Children are sorted largest first (the pinned remainder last), so the rank
    // among the first ring's children is the hue index.
    let mut rank = 0;
    for &child_ix in &node.children {
        let child = &tree.nodes[child_ix];
        let width = (span as f64 * child.size as f64 / total) as f32;
        let child_group = if depth == 1 {
            let g = if rank < HUES.len() { rank } else { NO_GROUP };
            rank += 1;
            g
        } else {
            group
        };
        if width < MIN_TURNS {
            let run = small.get_or_insert((angle, 0.0));
            run.1 += width;
            angle += width;
            continue;
        }
        flush(&mut small, out);
        out.push(Segment {
            target: Target::Node(child_ix),
            depth,
            start: angle,
            end: angle + width,
            kind: child.kind,
            group: child_group,
        });
        if child.kind == Kind::Dir && depth < MAX_DEPTH {
            layout_children(tree, child_ix, depth + 1, angle, angle + width, child_group, out);
        }
        angle += width;
    }
    flush(&mut small, out);
}

/// One of the six hues as HSL, so lightness can step with depth.
pub fn hue_color(index: usize) -> Hsla {
    rgb_to_hsla(HUES[index % HUES.len()])
}

/// A plain sRGB → HSL conversion; gpui's `Hsla` is in turns.
pub fn rgb_to_hsla(color: u32) -> Hsla {
    let r = ((color >> 16) & 0xff) as f32 / 255.0;
    let g = ((color >> 8) & 0xff) as f32 / 255.0;
    let b = (color & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta < 1e-6 {
        return hsla(0.0, 0.0, l, 1.0);
    }
    let s = delta / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    } / 6.0;
    hsla(h, s.clamp(0.0, 1.0), l, 1.0)
}

/// Colour by folder: the hue of the first-ring child the slice descends from,
/// a step lighter per ring so nested folders of one branch stay apart.
pub fn base_color(segment: &Segment) -> Hsla {
    let d = (segment.depth - 1) as f32;
    match (segment.target, segment.kind) {
        (Target::Small { .. }, _) => hsla(0.0, 0.0, 0.38, 1.0),
        (_, Kind::Other) => hsla(0.0, 0.0, 0.45, 1.0),
        _ if segment.group == NO_GROUP => hsla(0.0, 0.0, (0.48 + d * 0.04).min(0.7), 1.0),
        (_, Kind::File) => {
            let base = hue_color(segment.group);
            hsla(hue_turns(&base), base.saturation * 0.35, (base.lightness + 0.10 + d * 0.04).min(0.8), 1.0)
        }
        (_, Kind::Dir) => {
            let base = hue_color(segment.group);
            hsla(hue_turns(&base), (base.saturation * (1.0 - d * 0.08)).max(0.2), (base.lightness + d * 0.055).min(0.82), 1.0)
        }
    }
}

fn hue_turns(color: &Hsla) -> f32 {
    color.hue.into_positive_degrees() / 360.0
}

pub fn highlight(color: Hsla) -> Hsla {
    hsla(hue_turns(&color), (color.saturation + 0.1).min(1.0), (color.lightness + 0.14).min(0.9), 1.0)
}

pub fn dim(color: Hsla) -> Hsla {
    hsla(hue_turns(&color), color.saturation * 0.8, color.lightness * 0.82, 1.0)
}

/// Highlight for colours that are already vivid (colouring by kind): lighter, without
/// pushing saturation, which turns a pure green neon.
pub fn lift(color: Hsla) -> Hsla {
    hsla(hue_turns(&color), color.saturation.min(0.8), (color.lightness + 0.1).min(0.85), 1.0)
}

/// Much quieter than `dim`, so one highlighted group stands out from everything else.
pub fn fade(color: Hsla) -> Hsla {
    hsla(hue_turns(&color), color.saturation * 0.3, color.lightness * 0.6, 1.0)
}

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub center: Point<Pixels>,
    pub inner_radius: f32,
    /// Inner and outer radius of each ring.
    pub rings: [(f32, f32); MAX_DEPTH],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Center,
    Segment(usize),
}

impl Geometry {
    pub fn new(bounds: Bounds<Pixels>) -> Self {
        let radius = (f32::from(bounds.size.width).min(f32::from(bounds.size.height)) / 2.0 - 24.0)
            .max(40.0);
        let inner_radius = radius * 0.24;
        let weights: Vec<f32> = (0..MAX_DEPTH).map(|k| RING_FALLOFF.powi(k as i32)).collect();
        let unit = (radius - inner_radius) / weights.iter().sum::<f32>();
        let mut rings = [(0.0, 0.0); MAX_DEPTH];
        let mut r = inner_radius;
        for (k, weight) in weights.iter().enumerate() {
            rings[k] = (r, r + weight * unit);
            r += weight * unit;
        }
        Self { center: bounds.center(), inner_radius, rings }
    }

    /// A single ring, for small gauges.
    pub fn ring(center: Point<Pixels>, inner: f32, outer: f32) -> Self {
        Self { center, inner_radius: inner, rings: [(inner, outer); MAX_DEPTH] }
    }

    pub fn outer_radius(&self) -> f32 {
        self.rings[MAX_DEPTH - 1].1
    }

    pub fn hit_test(&self, position: Point<Pixels>, segments: &[Segment]) -> Option<Hit> {
        let dx = f32::from(position.x - self.center.x);
        let dy = f32::from(position.y - self.center.y);
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < self.inner_radius {
            return Some(Hit::Center);
        }
        let depth = self.rings.iter().position(|(r0, r1)| distance >= *r0 && distance < *r1)? + 1;
        let turns = dx.atan2(-dy).rem_euclid(TAU) / TAU;
        segments
            .iter()
            .position(|s| s.depth == depth && turns >= s.start && turns < s.end)
            .map(Hit::Segment)
    }

    fn at(&self, radius: f32, turns: f32) -> Point<Pixels> {
        let theta = turns * TAU;
        point(
            self.center.x + px(radius * theta.sin()),
            self.center.y - px(radius * theta.cos()),
        )
    }

    /// Paint an annular sector, leaving a hairline gap around it.
    pub fn paint_sector(&self, window: &mut Window, depth: usize, start: f32, end: f32, color: Hsla) {
        let (r0, r1) = self.rings[depth - 1];
        self.paint_band(window, r0, r1, start, end, color);
    }

    /// Inner and outer radius at a fractional ring depth (1.0 = first ring), so a
    /// segment can glide between rings.
    pub fn ring_at(&self, depth: f32) -> (f32, f32) {
        let depth = depth.clamp(1.0, MAX_DEPTH as f32);
        let below = depth.floor() as usize;
        let above = (below + 1).min(MAX_DEPTH);
        let t = depth - below as f32;
        let (a, b) = (self.rings[below - 1], self.rings[above - 1]);
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }

    /// Paint an annular sector between two radii, leaving a hairline gap around it.
    pub fn paint_band(&self, window: &mut Window, r0: f32, r1: f32, start: f32, end: f32, color: Hsla) {
        let (r0, r1) = (r0 + GAP_PX / 2.0, r1 - GAP_PX / 2.0);
        let span = end - start;
        let full_circle = span >= 0.9999;
        let inset = |r: f32| if full_circle { 0.0 } else { (GAP_PX / 2.0) / (r * TAU) };
        let (outer_start, outer_end) = (start + inset(r1), end - inset(r1));
        let (inner_start, inner_end) = (start + inset(r0), end - inset(r0));
        if outer_end <= outer_start || r1 <= r0 {
            return;
        }
        let (inner_start, inner_end) = if inner_end > inner_start {
            (inner_start, inner_end)
        } else {
            let mid = (start + end) / 2.0;
            (mid, mid)
        };

        let steps = ((span * 360.0 / 1.5).ceil() as usize).clamp(2, 400);
        let mut builder = PathBuilder::fill();
        builder.move_to(self.at(r1, outer_start));
        for i in 1..=steps {
            let t = outer_start + (outer_end - outer_start) * i as f32 / steps as f32;
            builder.line_to(self.at(r1, t));
        }
        for i in 0..=steps {
            let t = inner_end - (inner_end - inner_start) * i as f32 / steps as f32;
            builder.line_to(self.at(r0, t));
        }
        builder.close();
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }

    /// Diagonal hatching clipped to an annular sector, for slices staged for the
    /// Trash. gpui has no arbitrary clip, so each hatch line is sampled and only
    /// the runs inside the sector are stroked.
    pub fn paint_hatched_sector(&self, window: &mut Window, depth: usize, start: f32, end: f32, color: Hsla) {
        const SPACING: f32 = 6.0;
        const STEP: f32 = 1.5;
        let (r0, r1) = self.rings[depth - 1];
        let (r0, r1) = (r0 + GAP_PX, r1 - GAP_PX);
        if r1 <= r0 || end <= start {
            return;
        }
        // Tight bounding box: the sector's corners plus any axis crossing it spans.
        let cx: f32 = self.center.x.into();
        let cy: f32 = self.center.y.into();
        let mut xs = Vec::with_capacity(8);
        let mut ys = Vec::with_capacity(8);
        let mut push = |turns: f32, r: f32| {
            let theta = turns * TAU;
            xs.push(cx + r * theta.sin());
            ys.push(cy - r * theta.cos());
        };
        for &t in &[start, end] {
            push(t, r0);
            push(t, r1);
        }
        for q in 0..=4 {
            let t = q as f32 / 4.0;
            if t > start && t < end {
                push(t, r1);
            }
        }
        if end - start >= 0.9999 {
            push(0.0, r0);
        }
        let (x0, x1) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
        let (y0, y1) = (ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
        let inside = |x: f32, y: f32| {
            let (dx, dy) = (x - cx, y - cy);
            let d = (dx * dx + dy * dy).sqrt();
            if d < r0 || d > r1 {
                return false;
            }
            let turns = dx.atan2(-dy).rem_euclid(TAU) / TAU;
            (turns >= start && turns < end) || (end > 1.0 && turns < end - 1.0)
        };
        // Lines run at 45°: x - y = c. Walk c across the box, and each line along x.
        let mut builder = PathBuilder::stroke(px(1.0));
        let mut any = false;
        let c_from = ((x0 - y1) / SPACING).floor() * SPACING;
        let mut c = c_from;
        while c <= x1 - y0 {
            let mut x = x0;
            let mut run: Option<(f32, f32)> = None;
            while x <= x1 + STEP {
                let y = x - c;
                let on = y >= y0 && y <= y1 && inside(x, y);
                match (on, run) {
                    (true, None) => run = Some((x, y)),
                    (false, Some((sx, sy))) => {
                        let (ex, ey) = (x - STEP, y - STEP);
                        if ex > sx {
                            builder.move_to(point(px(sx), px(sy)));
                            builder.line_to(point(px(ex), px(ey)));
                            any = true;
                        }
                        run = None;
                    }
                    _ => {}
                }
                x += STEP;
            }
            c += SPACING;
        }
        if any {
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
    }

    /// Where a horizontal label fits inside a sector, and how wide it may be:
    /// the slice must be wide along the arc and thick across the ring, and the
    /// horizontal chord through its middle (bounded by the wedge near 12 and 6
    /// o'clock, by the ring near 3 and 9) must hold a few characters.
    pub fn label_slot(&self, depth: usize, start: f32, end: f32) -> Option<(Point<Pixels>, f32)> {
        const MIN_ARC: f32 = 54.0;
        const MIN_THICK: f32 = 14.0;
        let (r0, r1) = self.rings[depth - 1];
        let thickness = r1 - r0;
        let mid_r = (r0 + r1) / 2.0;
        let arc = (end - start) * TAU * mid_r;
        if arc < MIN_ARC || thickness < MIN_THICK {
            return None;
        }
        let theta = (start + end) / 2.0 * TAU;
        let (sin, cos) = (theta.sin().abs().max(0.05), theta.cos().abs().max(0.05));
        let horizontal = (thickness / sin).min(arc / cos);
        let vertical = (thickness / cos).min(arc / sin);
        if horizontal < 34.0 || vertical < 13.0 {
            return None;
        }
        Some((self.at(mid_r, (start + end) / 2.0), horizontal - 8.0))
    }

    pub fn paint_disc(&self, window: &mut Window, radius: f32, color: Hsla) {
        let mut builder = PathBuilder::fill();
        let steps = 180;
        builder.move_to(self.at(radius, 0.0));
        for i in 1..steps {
            builder.line_to(self.at(radius, i as f32 / steps as f32));
        }
        builder.close();
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Node;
    use std::path::PathBuf;

    fn node(name: &str, size: u64, kind: Kind, parent: Option<usize>, children: Vec<usize>) -> Node {
        Node { name: name.into(), size, kind, parent, children, items: 0 }
    }

    /// A large slice pinned after many thin ones (like "Not scanned yet") must keep its
    /// own segment instead of being folded into "smaller objects".
    #[test]
    fn large_slice_after_thin_ones_keeps_its_segment() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None, Vec::new())];
        let mut total = 0;
        for (i, size) in [600u64, 1, 1, 1, 400].into_iter().enumerate() {
            let kind = if i == 4 { Kind::Other } else { Kind::File };
            nodes.push(node(&format!("c{i}"), size, kind, Some(0), Vec::new()));
            nodes[0].children.push(i + 1);
            total += size;
        }
        nodes[0].size = total;
        let tree = Tree { root_path: PathBuf::from("/"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let segments = layout(&tree, Tree::ROOT);
        let targets: Vec<Target> = segments.iter().map(|s| s.target).collect();
        assert!(targets.contains(&Target::Node(5)), "the large last slice has its own segment: {targets:?}");
        let last = segments.iter().find(|s| s.target == Target::Node(5)).unwrap();
        assert!((last.end - 1.0).abs() < 1e-4 && last.end - last.start > 0.39);
    }
}
