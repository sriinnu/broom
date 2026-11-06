//! The drop: when a move to the Trash is confirmed, the ticked folders and files
//! tumble one by one into a ligne-claire bin while the real move runs. The lid
//! swings open for the first item and rattles as each one lands; the bin jolts
//! under the heavy ones and fills up, the last few poking out of the top. Once
//! the last item is in and the move has really finished, the lid slams shut
//! with a bounce and a burst of comic action lines. It never blocks the move
//! and never runs longer than about two seconds, however long the list.
//!
//! With Reduce Motion on, nothing flies, spins or bounces: the items fade out
//! of the queue one by one as the bin fills, and the lid simply shuts.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Bounds, ContentMask, Hsla, PathBuilder, Pixels, Point, Window, canvas, point, prelude::*, px, rgb, size};

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

/// `send`, and where each path landed in the home Trash, so it can be put
/// back. Neither Finder nor NSFileManager (through the `trash` crate) says,
/// so this lists `~/.Trash` before and after and matches the new entries by
/// name: the same name, or Finder's "name 18.33.12" (before any extension)
/// when the Trash already held one. Paths on other volumes go to that
/// volume's own Trash and simply get no entry.
pub fn send_tracked(paths: &[PathBuf]) -> (Result<(), String>, Vec<(PathBuf, PathBuf)>) {
    let Some(trash) = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".Trash")) else {
        return (send(paths), Vec::new());
    };
    let list = || -> std::collections::HashSet<std::ffi::OsString> {
        std::fs::read_dir(&trash).map(|dir| dir.filter_map(|e| e.ok().map(|e| e.file_name())).collect()).unwrap_or_default()
    };
    let before = list();
    let result = send(paths);
    let mut new: Vec<String> = list().difference(&before).filter_map(|n| n.to_str().map(str::to_owned)).collect();
    let mut landed = Vec::new();
    for path in paths {
        if path.symlink_metadata().is_ok() {
            continue; // still where it was: it never went
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let (stem, ext) = match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => (&name[..name.len() - ext.len() - 1], Some(ext)),
            None => (name, None),
        };
        let renamed = |n: &str| {
            let rest = match ext {
                Some(ext) => n.strip_suffix(ext).and_then(|n| n.strip_suffix('.')),
                None => Some(n),
            };
            rest.and_then(|r| r.strip_prefix(stem)).is_some_and(|r| r.starts_with(' '))
        };
        let found = new.iter().position(|n| n == name).or_else(|| new.iter().position(|n| renamed(n)));
        if let Some(i) = found {
            landed.push((path.clone(), trash.join(new.swap_remove(i))));
        }
    }
    (result, landed)
}

/// Move what `send_tracked` trashed back where it came from. Skips anything
/// whose old place is taken or whose folder is gone; returns what moved.
pub fn put_back(landed: &[(PathBuf, PathBuf)]) -> Vec<PathBuf> {
    landed
        .iter()
        .filter(|(original, in_trash)| {
            original.symlink_metadata().is_err()
                && original.parent().is_some_and(Path::is_dir)
                && std::fs::rename(in_trash, original).is_ok()
        })
        .map(|(original, _)| original.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Emptying the Trash
// ---------------------------------------------------------------------------

/// Every Trash that Finder's Empty Trash would empty: the home one, and the
/// `.Trashes/<uid>` folder on each other mounted volume (what was trashed
/// from there went there). Only the ones that exist.
pub fn trash_dirs(home: &Path, volumes: &[PathBuf]) -> Vec<PathBuf> {
    let uid = unsafe { libc::getuid() };
    std::iter::once(home.join(".Trash"))
        .chain(volumes.iter().map(|v| v.join(".Trashes").join(uid.to_string())))
        .filter(|d| d.is_dir())
        .collect()
}

/// What's in the Trash: top-level items, and the bytes they take on disk
/// (allocated blocks, a hard-linked file once), which is what emptying frees
/// barring clones shared with files outside the Trash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub items: usize,
    pub bytes: u64,
}

/// Why the Trash couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// macOS keeps the Trash behind Full Disk Access.
    NoAccess,
    Other(String),
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>, Unreadable> {
    match std::fs::read_dir(dir) {
        Ok(read) => Ok(read.filter_map(|e| e.ok().map(|e| e.path())).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(libc::EPERM) => Err(Unreadable::NoAccess),
        Err(e) => Err(Unreadable::Other(format!("{}: {e}", dir.display()))),
    }
}

/// The first folder is the home Trash: if that can't be read, nothing can be
/// said. Another volume's Trash that can't be read is passed over, so it
/// never stops the rest half way.
fn listing(i: usize, dir: &Path) -> Result<Vec<PathBuf>, Unreadable> {
    match entries(dir) {
        Err(e) if i == 0 => Err(e),
        Err(_) => Ok(Vec::new()),
        ok => ok,
    }
}

/// Bytes on disk under `path`, not following links; inodes already seen
/// (hard links) count once.
fn allocated(path: &Path, seen: &mut std::collections::HashSet<(u64, u64)>) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = path.symlink_metadata() else { return 0 };
    if meta.nlink() > 1 && !meta.is_dir() && !seen.insert((meta.dev(), meta.ino())) {
        return 0;
    }
    let mut bytes = meta.blocks() * 512;
    if meta.is_dir() {
        if let Ok(read) = std::fs::read_dir(path) {
            for entry in read.flatten() {
                bytes += allocated(&entry.path(), seen);
            }
        }
    }
    bytes
}

/// Count what's in these Trash folders. `.DS_Store` is Finder's, not the user's.
pub fn measure(dirs: &[PathBuf]) -> Result<Contents, Unreadable> {
    let mut seen = std::collections::HashSet::new();
    let mut contents = Contents::default();
    for (i, dir) in dirs.iter().enumerate() {
        for path in listing(i, dir)? {
            let bytes = allocated(&path, &mut seen);
            if path.file_name().is_some_and(|n| n == ".DS_Store") {
                continue;
            }
            contents.items += 1;
            contents.bytes += bytes;
        }
    }
    Ok(contents)
}

/// What emptying did.
#[derive(Clone, Debug, Default)]
pub struct Emptied {
    /// What went, counted as `measure` counts.
    pub gone: Contents,
    /// Top-level items that stayed (locked, in use, no permission), by name.
    pub stuck: Vec<String>,
}

/// Delete everything in these Trash folders, for good. Each top-level item is
/// measured, then removed; one that won't go (a locked file, say: Finder
/// would ask about those) stays, whole or in part, and is reported. Partly
/// removed folders count only the part that went.
pub fn empty(dirs: &[PathBuf]) -> Result<Emptied, Unreadable> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Emptied::default();
    for (i, dir) in dirs.iter().enumerate() {
        for path in listing(i, dir)? {
            let user = path.file_name().is_none_or(|n| n != ".DS_Store");
            let before = allocated(&path, &mut seen);
            let is_dir = path.symlink_metadata().is_ok_and(|m| m.is_dir());
            let result = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            if !user {
                continue;
            }
            match result {
                Ok(()) => {
                    out.gone.items += 1;
                    out.gone.bytes += before;
                }
                Err(_) => {
                    // What's left of it is what didn't go; measure it fresh.
                    let left = allocated(&path, &mut std::collections::HashSet::new());
                    out.gone.bytes += before.saturating_sub(left);
                    out.stuck.push(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                }
            }
        }
    }
    Ok(out)
}

/// Without Full Disk Access Broom can't see into the Trash, but Finder can
/// empty it. macOS asks once whether Broom may tell Finder to.
pub fn empty_with_finder() -> Result<(), String> {
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "tell application \"Finder\" to empty trash"])
        .output()
        .map_err(|e| format!("couldn't ask Finder: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Free bytes on the volume holding each path, deduplicated by volume.
pub fn free_space(paths: &[PathBuf]) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut devices = std::collections::HashSet::new();
    paths
        .iter()
        .filter(|p| p.metadata().is_ok_and(|m| devices.insert(m.dev())))
        .filter_map(|p| crate::scan::statvfs(p).map(|(_, free)| free))
        .sum()
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
/// closes, and the bounce after (the action lines fit inside it).
const LID_OPEN: f32 = 0.24;
const FIRST: f32 = 0.12;
const FLIGHT: f32 = 0.42;
const CLOSE: f32 = 0.2;
const BOUNCE: f32 = 0.36;
/// Reduce Motion: each item's fade, and how long the shut bin stays in view.
const FADE: f32 = 0.18;
const HOLD: f32 = 0.25;
/// How long a landing shakes the bin and rattles the lid.
const JOLT: f32 = 0.22;
/// Lanes across the top of the stage that items wait in.
const SLOTS: usize = 10;
/// The most items that take part. A longer list is sampled evenly down to
/// this, and the count under the bin scales back up to the real one: a
/// thousand folders can't each have a turn in under two seconds.
const SHOWN: usize = 40;
/// Things poking out of the top of a full bin: where across the mouth (in
/// rim radii), their tilt, and at what fill each one shows.
const POKES: [(f32, f32, f32); 5] = [(-0.42, -0.38, 0.2), (0.3, 0.3, 0.4), (-0.08, 0.06, 0.6), (0.52, 0.55, 0.78), (-0.6, -0.2, 0.92)];

pub struct Item {
    pub kind: Kind,
    /// 0.6..1: bigger things draw bigger, and land harder.
    pub weight: f32,
}

impl Item {
    /// One item per node; the weight follows the log of its size against the largest.
    pub fn from_sizes(items: &[(Kind, u64)]) -> Vec<Item> {
        let max = items.iter().map(|&(_, s)| s).max().unwrap_or(1).max(1) as f32;
        items
            .iter()
            .map(|&(kind, size)| {
                // A lone item (or all one size) is the heaviest there is.
                let rel = if max <= 1.0 { 1.0 } else { ((size.max(1) as f32).ln() / max.ln()).clamp(0.0, 1.0) };
                Item { kind, weight: 0.6 + 0.4 * rel }
            })
            .collect()
    }
}

pub struct Drop {
    start: Instant,
    /// The ones that fly; at most `SHOWN`.
    items: Vec<Item>,
    /// How many there really are.
    count: usize,
    stagger: f32,
    bytes: u64,
    /// Seconds after `start` when the real move finished; the lid can't close before.
    settled: Cell<Option<f32>>,
    /// Reduce Motion, read once when the drop starts so it can't change mid-way.
    still: bool,
}

impl Drop {
    pub fn new(items: Vec<Item>, bytes: u64) -> Rc<Self> {
        Rc::new(Self::with_motion(items, bytes, crate::delight::reduce_motion()))
    }

    fn with_motion(items: Vec<Item>, bytes: u64, still: bool) -> Self {
        let count = items.len();
        let items: Vec<Item> = if count <= SHOWN {
            items
        } else {
            let mut items: Vec<Option<Item>> = items.into_iter().map(Some).collect();
            (0..SHOWN).filter_map(|k| items[k * count / SHOWN].take()).collect()
        };
        let n = items.len().max(1);
        let stagger = (0.55 / n as f32).min(0.1);
        Drop { start: clock::now(), items, count, stagger, bytes, settled: Cell::new(None), still }
    }

    fn t(&self) -> f32 {
        clock::since(self.start).as_secs_f32()
    }

    fn launch(&self, i: usize) -> f32 {
        FIRST + i as f32 * self.stagger
    }

    /// When item `i` is in the bin: the end of its flight, or of its fade.
    fn land(&self, i: usize) -> f32 {
        self.launch(i) + if self.still { FADE } else { FLIGHT }
    }

    fn last_land(&self) -> f32 {
        self.land(self.items.len().saturating_sub(1))
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

    /// From the lid starting to close to the end of the show.
    fn closing(&self) -> f32 {
        if self.still { HOLD } else { CLOSE + BOUNCE }
    }

    /// After `settle`: how long until the lid is shut and the bounce is over.
    pub fn remaining(&self) -> Duration {
        match self.close_at() {
            Some(close) => Duration::from_secs_f32((close + self.closing() - self.t()).max(0.0)),
            None => Duration::ZERO,
        }
    }

    fn finished_at(&self, t: f32) -> bool {
        self.close_at().is_some_and(|close| t >= close + self.closing())
    }

    /// Items in the bin so far, and the bytes they stand for, counted in
    /// real items however few of them are drawn.
    pub fn landed(&self) -> (usize, u64) {
        self.landed_at(self.t())
    }

    fn landed_at(&self, t: f32) -> (usize, u64) {
        let n = (0..self.items.len()).filter(|&i| t >= self.land(i)).count();
        let fraction = if self.items.is_empty() { 1.0 } else { n as f64 / self.items.len() as f64 };
        ((self.count as f64 * fraction).round() as usize, (self.bytes as f64 * fraction) as u64)
    }

    pub fn total(&self) -> usize {
        self.count
    }

    /// How open the lid is, 0 (shut) to about 1.1 (the overshoot as it swings
    /// back), plus a rattle in radians while things land.
    fn lid(&self, t: f32) -> (f32, f32) {
        let shut = self.close_at().map(|close| (t - close) / if self.still { f32::EPSILON } else { CLOSE });
        if self.still {
            // No swing: open from the first frame, shut the moment it's done.
            return (if shut.is_some_and(|k| k >= 0.0) { 0.0 } else { 1.0 }, 0.0);
        }
        let mut open = back_out(t / LID_OPEN).max(0.0);
        if let Some(k) = shut {
            // Accelerating into the slam.
            let k = k.clamp(0.0, 1.0);
            open *= 1.0 - k * k;
        }
        let rattle = self.recent(t).map(|(u, w)| 0.07 * w * (u * 26.0).sin() * (1.0 - u)).sum::<f32>();
        (open, if open > 0.05 { rattle.clamp(-0.12, 0.12) } else { 0.0 })
    }

    /// Landings in the last `JOLT` seconds: how far through (0..1) and how heavy.
    fn recent(&self, t: f32) -> impl Iterator<Item = (f32, f32)> + '_ {
        // Only the last few launched can be recent; scan back from there.
        let last = ((t - FIRST - FLIGHT) / self.stagger).floor().max(0.0) as usize;
        let from = last.saturating_sub((JOLT / self.stagger).ceil() as usize + 1);
        (from..=last.min(self.items.len().saturating_sub(1)))
            .filter(move |_| !self.items.is_empty())
            .filter_map(move |i| {
                let u = (t - self.land(i)) / JOLT;
                (0.0..1.0).contains(&u).then(|| (u, self.items[i].weight))
            })
    }

    /// How squashed the bin is: a jolt as each item lands (the heavier, the
    /// harder), and the bounce once the lid has slammed.
    fn squash(&self, t: f32) -> f32 {
        if self.still {
            return 0.0;
        }
        let jolt = self
            .recent(t)
            .map(|(u, w)| 0.05 * (w - 0.5) * 2.0 * (std::f32::consts::PI * u).sin() * (1.0 - u))
            .fold(0.0f32, f32::max);
        let bounce = self.close_at().map_or(0.0, |close| {
            let u = (t - close - CLOSE) / BOUNCE;
            if u > 0.0 && u < 1.0 { 0.09 * (std::f32::consts::PI * u).sin() * (1.0 - u) } else { 0.0 }
        });
        jolt.max(bounce)
    }

    /// How full the bin looks, 0..1, rising smoothly as each item lands.
    fn fill(&self, t: f32) -> f32 {
        if self.items.is_empty() {
            return 0.0;
        }
        let rise = if self.still { FADE } else { 0.15 };
        let sum: f32 = (0..self.items.len()).map(|i| ((t - self.land(i)) / rise).clamp(0.0, 1.0)).sum();
        sum / self.items.len() as f32
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

fn with_alpha(color: u32, alpha: f32) -> Hsla {
    let mut c = to_hsla(color);
    c.alpha = alpha;
    c
}

/// Marks drawn on the card itself rather than on a shape (speed lines,
/// action lines, the shadow): ink on a light card, cream on a dark one, or
/// they vanish into it.
fn marks(alpha: f32) -> Hsla {
    with_alpha(if crate::theme::is_dark() { CREAM } else { INK }, alpha)
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Ease out past the end and back: the lid swings a touch too far, then settles.
fn back_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0) - 1.0;
    let c = 1.9;
    1.0 + (c + 1.0) * x * x * x + c * x * x
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

/// A file or folder; `alpha` below 1 fades the whole thing, ink and all.
fn icon(window: &mut Window, kind: Kind, at: (f32, f32), scale: f32, angle: f32, alpha: f32) {
    let ink = with_alpha(INK, alpha);
    match kind {
        Kind::File => {
            piece(window, &PAPER, at, scale, angle, with_alpha(CREAM, alpha), ink);
            let ear = place(&PAPER_EAR, at, scale, angle);
            fill(window, &ear, with_alpha(PAPER_FOLD, alpha));
            // Two lines of "text".
            for (i, y) in [(-1.5f32), 2.5].iter().enumerate() {
                let w = if i == 0 { 9.0 } else { 6.0 };
                let a = place(&[(-4.5, *y), (-4.5 + w, *y)], at, scale, angle);
                line(window, a[0], a[1], 1.2 * scale, with_alpha(PAPER_FOLD, alpha));
            }
        }
        _ => {
            piece(window, &FOLDER_TAB_SHAPE, at, scale, angle, with_alpha(FOLDER_TAB, alpha), ink);
            piece(window, &FOLDER_BODY, at, scale, angle, with_alpha(FOLDER, alpha), ink);
        }
    }
}

/// A comic dust puff: a little cloud, cream with an ink contour, swelling and fading.
fn puff(window: &mut Window, center: (f32, f32), radius: f32, alpha: f32, s: f32) {
    let shape = ellipse(center.0, center.1, radius, radius * 0.85, 0.0, 1.0);
    fill(window, &shape, with_alpha(CREAM, alpha));
    outline(window, &shape, 1.1 * s, with_alpha(INK, alpha * 0.8));
}

fn paint(window: &mut Window, bounds: Bounds<Pixels>, drop: &Drop) {
    let t = drop.t();
    let ox: f32 = bounds.origin.x.into();
    let oy: f32 = bounds.origin.y.into();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    let ink = to_hsla(INK);
    let pi = std::f32::consts::PI;
    // The bin is drawn in a 100-unit-tall box and scaled to the stage.
    let s = (h / 150.0).clamp(0.6, 1.6);
    let cx = ox + w / 2.0;

    let (open, rattle) = drop.lid(t);
    let squash = drop.squash(t);
    let fullness = drop.fill(t);
    let floor = oy + h - 8.0 * s;
    let body_h = 62.0 * s * (1.0 - squash);
    let top_y = floor - body_h;
    let widen = 1.0 + squash * 0.8;
    let (rx_top, rx_bottom) = (30.0 * s * widen, 25.0 * s * widen);
    let ry = 6.5 * s;
    let mouth_y = top_y;

    // A flat shadow on the floor, spreading as the bin squashes.
    let shadow = ellipse(cx, floor + 1.5 * s, rx_bottom * 1.3 * widen, 4.0 * s, 0.0, 1.0);
    fill(window, &shadow, if crate::theme::is_dark() { with_alpha(0x000000, 0.35) } else { with_alpha(INK, 0.16) });

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

    // Everything that goes into the bin is cut off a little below the rim, so
    // the front lip hides where it enters and nothing draws over the body.
    let into_bin = ContentMask { bounds: Bounds::new(bounds.origin, size(bounds.size.width, px(mouth_y + ry * 0.5 - oy))) };
    let shelf_y = oy + 16.0 * s;
    let lane = |i: usize| ox + w * (0.08 + 0.84 * ((i % SLOTS) as f32 + 0.5) / SLOTS as f32);
    window.with_content_mask(Some(into_bin), |window| {
        // What's in already, poking out of the top as the bin fills.
        let n = drop.items.len();
        for (j, &(across, tilt, at)) in POKES.iter().enumerate() {
            let rise = smooth((fullness - at) / 0.15);
            if rise <= 0.0 || n == 0 {
                continue;
            }
            let item = &drop.items[(j * n / POKES.len()).min(n - 1)];
            let y = mouth_y + ry * 0.5 + 6.0 * s - rise * 10.0 * s;
            icon(window, item.kind, (cx + across * rx_top, y), s * 0.62, tilt + rattle, 1.0);
        }

        // The queue along the top, and the items on their way in.
        for (i, item) in drop.items.iter().enumerate() {
            let k = (t - drop.launch(i)) / FLIGHT;
            let scale = s * 0.9 * item.weight;
            let waiting = t < drop.launch(i);
            if drop.still {
                // Reduce Motion: no flight, just a fade out of the queue.
                let fade = 1.0 - ((t - drop.launch(i)) / FADE).clamp(0.0, 1.0);
                if fade > 0.0 && i < SLOTS * 2 {
                    let stack = (i / SLOTS) as f32;
                    icon(window, item.kind, (lane(i), shelf_y - stack * 6.0 * s), scale, 0.0, fade);
                }
                continue;
            }
            if waiting {
                // Only the next few lanes' worth, so a long list doesn't pile up.
                if i >= SLOTS * 2 {
                    continue;
                }
                let stack = (i / SLOTS) as f32;
                let wobble = (t * 3.0 + i as f32).sin() * 0.03;
                // A crouch just before the jump.
                let crouch = smooth(1.0 - (drop.launch(i) - t) / 0.08) * 2.0 * s;
                icon(window, item.kind, (lane(i), shelf_y - stack * 6.0 * s + crouch), scale, wobble, 1.0);
            } else if k < 1.0 {
                let x0 = lane(i);
                let x1 = cx + ((i % 5) as f32 - 2.0) * 3.0 * s;
                let ex = smooth(k);
                let x = lerp(x0, x1, ex);
                let hop = 18.0 * s;
                let fall = |k: f32| shelf_y + (mouth_y + 4.0 * s - shelf_y) * k * k - hop * (pi * k).sin();
                let y = fall(k);
                let spin = (0.9 + (i % 3) as f32 * 0.5) * if i % 2 == 0 { 1.0 } else { -1.0 };
                let shrink = if k > 0.75 { lerp(1.0, 0.7, (k - 0.75) / 0.25) } else { 1.0 };
                // Speed lines trailing the throw, the comic way of saying "fast".
                if (0.12..0.8).contains(&k) {
                    let dk = 0.04;
                    let (bx, by) = (lerp(x0, x1, smooth(k - dk)), fall(k - dk));
                    let (dx, dy) = (x - bx, y - by);
                    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
                    let (ux, uy) = (dx / len, dy / len);
                    let fade = (1.0 - (k - 0.12) / 0.68).clamp(0.0, 1.0) * 0.55;
                    for side in [-1.0f32, 1.0] {
                        let off = side * 4.5 * s * item.weight;
                        let start = (x - ux * 12.0 * s - uy * off, y - uy * 12.0 * s + ux * off);
                        let end = (start.0 - ux * 9.0 * s, start.1 - uy * 9.0 * s);
                        line(window, point(px(start.0), px(start.1)), point(px(end.0), px(end.1)), 1.2 * s, marks(fade));
                    }
                }
                icon(window, item.kind, (x, y), scale * shrink, k * spin, 1.0);
            }
        }
    });

    // Dust where each item went in, above the rim.
    if !drop.still {
        for i in 0..drop.items.len() {
            let since = t - drop.land(i);
            if !(0.0..0.3).contains(&since) {
                continue;
            }
            let u = since / 0.3;
            let alpha = 0.85 * (1.0 - u);
            let wgt = drop.items[i].weight;
            for j in 0..3 {
                let dx = ((j as f32) - 1.0) * (7.0 + 10.0 * u) * s * wgt;
                let r = (3.2 - 1.2 * u) * s * wgt;
                puff(window, (cx + dx, mouth_y - (3.0 + 9.0 * u + if j == 1 { 3.0 } else { 0.0 }) * s), r, alpha, s);
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
    let angle = -1.3 * open + rattle;
    let span = 2.0 * rx_top / s + 2.0;
    let lid: [(f32, f32); 6] = [(0.0, 0.0), (span, 0.0), (span - 2.0, -6.0), (span * 0.8, -8.5), (span * 0.2, -8.5), (2.0, -6.0)];
    piece(window, &lid, hinge, s, angle, to_hsla(CAN_LIGHT), ink);
    // The lid's rim: a darker band along its underside.
    let band = place(&[(2.5, -2.2), (span - 2.5, -2.2)], hinge, s, angle);
    line(window, band[0], band[1], 1.4 * s, to_hsla(CAN));
    let knob = place(&[(span / 2.0, -11.0)], hinge, s, angle);
    disc(window, knob[0], (3.2 + CONTOUR) * s, ink);
    disc(window, knob[0], 3.2 * s, to_hsla(CAN_LIGHT));

    // The slam: action lines bursting out around the shut lid.
    if let (false, Some(close)) = (drop.still, drop.close_at()) {
        let u = (t - close - CLOSE * 0.85) / 0.3;
        if (0.0..1.0).contains(&u) {
            let alpha = 1.0 - u;
            let centre = (cx, mouth_y - 6.0 * s);
            for k in 0..7 {
                // A fan over the lid, from the left round to the right.
                let a = pi + pi * (k as f32 + 0.5) / 7.0;
                let (r0, r1) = ((rx_top / s + 6.0 + 10.0 * u) * s, (rx_top / s + 12.0 + 16.0 * u) * s);
                let (sin, cos) = a.sin_cos();
                let ry0 = 0.55;
                line(
                    window,
                    point(px(centre.0 + r0 * cos), px(centre.1 + r0 * sin * ry0)),
                    point(px(centre.0 + r1 * cos), px(centre.1 + r1 * sin * ry0)),
                    1.6 * s,
                    marks(alpha * 0.9),
                );
            }
        }
    }

    if !drop.finished_at(t) {
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("broom-empty-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn measures_items_once_and_skips_finders_ds_store() {
        let trash = temp("measure");
        std::fs::write(trash.join("a.bin"), vec![1u8; 100_000]).unwrap();
        std::fs::create_dir_all(trash.join("folder/inner")).unwrap();
        std::fs::write(trash.join("folder/inner/b.bin"), vec![2u8; 50_000]).unwrap();
        // A hard link to a.bin inside the folder: its blocks are counted once.
        std::fs::hard_link(trash.join("a.bin"), trash.join("folder/link.bin")).unwrap();
        std::fs::write(trash.join(".DS_Store"), vec![0u8; 6_000]).unwrap();
        let contents = measure(std::slice::from_ref(&trash)).unwrap();
        assert_eq!(contents.items, 2);
        assert!(contents.bytes >= 150_000 && contents.bytes < 220_000, "{}", contents.bytes);
        std::fs::remove_dir_all(&trash).ok();
    }

    #[test]
    fn empties_everything_and_reports_what_stayed() {
        let trash = temp("empty");
        std::fs::write(trash.join("a.bin"), vec![1u8; 40_000]).unwrap();
        std::fs::create_dir_all(trash.join("folder")).unwrap();
        std::fs::write(trash.join("folder/b.bin"), vec![2u8; 40_000]).unwrap();
        std::fs::write(trash.join("folder/locked.bin"), vec![3u8; 40_000]).unwrap();
        std::fs::write(trash.join(".DS_Store"), b"x").unwrap();
        // Lock one file, as Finder's Get Info "Locked" does.
        let locked = std::ffi::CString::new(trash.join("folder/locked.bin").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::chflags(locked.as_ptr(), libc::UF_IMMUTABLE as _) }, 0);
        let before = measure(std::slice::from_ref(&trash)).unwrap();
        let emptied = empty(std::slice::from_ref(&trash)).unwrap();
        assert_eq!(emptied.gone.items, 1, "a.bin went whole");
        assert_eq!(emptied.stuck, vec!["folder".to_string()]);
        assert!(!trash.join("a.bin").exists() && !trash.join(".DS_Store").exists());
        assert!(!trash.join("folder/b.bin").exists(), "the rest of the folder went");
        assert!(trash.join("folder/locked.bin").exists());
        assert!(emptied.gone.bytes >= 80_000 && emptied.gone.bytes < before.bytes);
        assert_eq!(unsafe { libc::chflags(locked.as_ptr(), 0) }, 0);
        std::fs::remove_dir_all(&trash).ok();
    }

    #[test]
    fn an_unreadable_trash_says_so() {
        let missing = temp("missing").join("nope");
        assert!(matches!(measure(&[missing]), Err(Unreadable::Other(_))));
    }

    #[test]
    fn trash_dirs_lists_the_ones_that_exist() {
        let home = temp("home");
        std::fs::create_dir_all(home.join(".Trash")).unwrap();
        let vol = temp("vol");
        let uid = unsafe { libc::getuid() };
        std::fs::create_dir_all(vol.join(".Trashes").join(uid.to_string())).unwrap();
        let bare = temp("bare");
        let dirs = trash_dirs(&home, &[vol.clone(), bare.clone()]);
        assert_eq!(dirs, vec![home.join(".Trash"), vol.join(".Trashes").join(uid.to_string())]);
        for d in [home, vol, bare] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n).map(|i| Item { kind: if i % 2 == 0 { Kind::Dir } else { Kind::File }, weight: 0.6 + 0.4 * (i % 3) as f32 / 2.0 }).collect()
    }

    #[test]
    fn weights_follow_size_and_a_lone_item_is_heaviest() {
        let w = Item::from_sizes(&[(Kind::Dir, 1 << 30), (Kind::File, 1 << 10), (Kind::File, 0)]);
        assert!((w[0].weight - 1.0).abs() < 1e-6);
        assert!(w[1].weight < w[0].weight && w[1].weight >= 0.6);
        assert!((w[2].weight - 0.6).abs() < 1e-6);
        assert!((Item::from_sizes(&[(Kind::File, 1)])[0].weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lid_opens_with_an_overshoot_and_shuts_after_the_last_landing() {
        let drop = Drop::with_motion(items(4), 100, false);
        assert_eq!(drop.lid(0.0).0, 0.0);
        let peak = (1..50).map(|i| drop.lid(i as f32 * LID_OPEN / 50.0).0).fold(0.0, f32::max);
        assert!(peak > 1.02, "swings past open, got {peak}");
        assert!((drop.lid(LID_OPEN).0 - 1.0).abs() < 1e-4);
        // Settled early: the lid still waits for the last item.
        drop.settled.set(Some(0.0));
        let close = drop.close_at().unwrap();
        assert!(close >= drop.last_land());
        assert!(drop.lid(close - 0.01).0 > 0.5);
        assert_eq!(drop.lid(close + CLOSE).0, 0.0);
        assert!(!drop.finished_at(close + CLOSE));
        assert!(drop.finished_at(close + CLOSE + BOUNCE));
    }

    #[test]
    fn landings_jolt_the_bin_and_fill_it() {
        let drop = Drop::with_motion(items(3), 100, false);
        assert_eq!(drop.fill(0.0), 0.0);
        let land = drop.land(0);
        assert_eq!(drop.squash(land - 0.01), 0.0);
        assert!(drop.squash(land + JOLT * 0.3) > 0.0, "heavy landing squashes the bin");
        assert!(drop.lid(land + 0.02).1.abs() > 0.0, "and rattles the lid");
        assert!((drop.fill(drop.last_land() + 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reduce_motion_never_moves_the_bin_and_ends_sooner() {
        let still = Drop::with_motion(items(20), 100, true);
        let moving = Drop::with_motion(items(20), 100, false);
        for i in 0..200 {
            let t = i as f32 * 0.01;
            assert_eq!(still.squash(t), 0.0);
            assert_eq!(still.lid(t).1, 0.0);
            assert!(still.lid(t).0 == 1.0, "open the whole time, no swing");
        }
        still.settled.set(Some(0.0));
        moving.settled.set(Some(0.0));
        let end = |d: &Drop| d.close_at().unwrap() + d.closing();
        assert!(end(&still) < end(&moving));
        assert_eq!(still.lid(still.close_at().unwrap()).0, 0.0);
    }

    #[test]
    fn a_long_list_still_fits_in_under_two_seconds() {
        let drop = Drop::with_motion(items(5000), 100, false);
        drop.settled.set(Some(0.0));
        let end = drop.close_at().unwrap() + drop.closing();
        assert!(end < 1.9, "took {end}");
        // Forty fly, and the count still reaches all five thousand.
        assert_eq!(drop.items.len(), SHOWN);
        assert_eq!(drop.total(), 5000);
        assert_eq!(drop.landed_at(0.0), (0, 0));
        assert_eq!(drop.landed_at(drop.last_land()), (5000, 100));
        let (half, _) = drop.landed_at(drop.land(SHOWN / 2 - 1));
        assert_eq!(half, 2500);
    }
}
