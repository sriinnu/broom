//! The small joys: what a cleanup is worth in everyday things, the freed number
//! counting up, a puff of dust where a slice used to be, light asides while a
//! scan runs, and a lifetime tally of what Broom has swept.
//!
//! Everything here is either pure (and tested) or a thin read of the system,
//! so the screens only decide *where* a touch goes, never how it behaves. None
//! of it delays an action: animations run after the work is done and are
//! short, and macOS Reduce Motion turns the moving ones off.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::findings;
use crate::scan::format_size;
use crate::settings;

// ---------------------------------------------------------------------------
// Reduce Motion
// ---------------------------------------------------------------------------

/// Whether macOS asks apps to cut back on motion (System Settings ›
/// Accessibility › Display › Reduce motion). `BROOM_REDUCE_MOTION=1` (or `0`)
/// overrides it, so screenshots can show both.
///
/// Read at most every two seconds: it's an Objective-C call and the charts ask
/// on every frame, but someone flipping the switch should see it take effect
/// without a relaunch.
pub fn reduce_motion() -> bool {
    if let Some(value) = std::env::var_os("BROOM_REDUCE_MOTION") {
        return value != "0";
    }
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match *cache {
        Some((at, value)) if at.elapsed() < Duration::from_secs(2) => value,
        _ => {
            let value = system_reduce_motion();
            *cache = Some((Instant::now(), value));
            value
        }
    }
}

#[cfg(not(test))]
fn system_reduce_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Tests run headless and in parallel; the answer there is "no".
#[cfg(test)]
fn system_reduce_motion() -> bool {
    false
}

// ---------------------------------------------------------------------------
// What a cleanup is worth
// ---------------------------------------------------------------------------

/// Everyday things with a typical size (decimal bytes, as `format_size` counts).
/// Rough on purpose: the point is a feel for the number, not a spec sheet.
const THINGS: &[(u64, &str, &str)] = &[
    (75_000, "an email", "emails"),
    (1_440_000, "a floppy disk", "floppy disks"),
    (3_000_000, "a phone photo", "phone photos"),
    (8_000_000, "a song", "songs"),
    (170_000_000, "a minute of 4K video", "minutes of 4K video"),
    (700_000_000, "a CD", "CDs"),
    (4_000_000_000, "an HD movie", "HD movies"),
    (15_000_000_000, "a whole season of a show", "seasons of a show"),
];

/// Round a count the way people say it: exact under 20, then to the nearest
/// 5, then the nearest 50. "About 437 songs" reads as false precision.
fn round_nice(n: f64) -> u64 {
    let n = n.round() as u64;
    match n {
        0..20 => n,
        20..100 => (n + 2) / 5 * 5,
        _ => (n + 25) / 50 * 50,
    }
}

/// "about 900 phone photos" for this many bytes, or `None` when it's too small
/// to compare with anything. Only things that give a count people can picture
/// (about one up to under a thousand) are candidates; which one is used varies
/// with the size, so two cleanups rarely read the same, but a given size
/// always reads the same way.
pub fn comparison(bytes: u64) -> Option<String> {
    let candidates: Vec<(f64, &str, &str)> = THINGS
        .iter()
        .map(|&(size, one, many)| (bytes as f64 / size as f64, one, many))
        .filter(|(ratio, _, _)| (0.9..1000.0).contains(ratio))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let (ratio, one, many) = candidates[(bytes / 1000) as usize % candidates.len()];
    let n = round_nice(ratio);
    Some(if n <= 1 { format!("about {one}") } else { format!("about {} {many}", crate::scan::format_count(n)) })
}

// ---------------------------------------------------------------------------
// The freed number, counting up
// ---------------------------------------------------------------------------

/// How long the freed bytes take to count up in the banner.
pub const COUNT_UP: Duration = Duration::from_millis(600);

/// The value shown `elapsed` into the count-up: fast at first, settling on
/// `target` (ease-out cubic), and exactly `target` once the time is up.
pub fn counted(target: u64, elapsed: Duration) -> u64 {
    let t = (elapsed.as_secs_f64() / COUNT_UP.as_secs_f64()).clamp(0.0, 1.0);
    if t >= 1.0 {
        return target;
    }
    let eased = 1.0 - (1.0 - t).powi(3);
    (target as f64 * eased) as u64
}

/// `value` written in the unit and with the decimals `format_size(target)`
/// uses, padded with figure spaces to the same length, so the number can count
/// up in place without the banner around it changing width.
pub fn format_like(value: u64, target: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let full = format_size(target);
    if target < 1000 {
        return pad(format!("{} bytes", value.min(target)), &full);
    }
    let mut unit = 0;
    let mut scale = 1000.0;
    let mut v = target as f64 / 1000.0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        scale *= 1000.0;
        unit += 1;
    }
    let decimals = if v >= 100.0 { 0 } else if v >= 10.0 { 1 } else { 2 };
    let shown = value.min(target) as f64 / scale;
    pad(format!("{shown:.decimals$} {}", UNITS[unit]), &full)
}

fn pad(text: String, like: &str) -> String {
    let missing = like.chars().count().saturating_sub(text.chars().count());
    // U+2007 FIGURE SPACE: as wide as a digit in tabular-figure fonts.
    "\u{2007}".repeat(missing) + &text
}

// ---------------------------------------------------------------------------
// Dust where a slice used to be
// ---------------------------------------------------------------------------

/// The dust's colour: the same warm grey the investigator kicks up.
pub const DUST_COLOR: u32 = 0x9a9ea6;

/// How long the dust drifts and fades after a move to the Trash.
pub const DUST: Duration = Duration::from_millis(550);
/// More than this many specks reads as noise rather than a puff.
const MAX_SPECKS: usize = 42;

/// One mote of dust, in chart terms: where around the circle (turns from 12
/// o'clock, clockwise), how far across its ring (0 inner edge, 1 outer), which
/// ring, how far it drifts outward and up, and how big it starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speck {
    pub turns: f32,
    pub across: f32,
    pub depth: usize,
    pub drift: f32,
    pub size: f32,
}

/// A puff of dust over the slices that just went to the Trash.
pub struct Puff {
    pub start: Instant,
    pub specks: Vec<Speck>,
}

/// A small, fixed sequence of pseudo-random numbers: the same slices always
/// puff the same way, which keeps screenshots and tests repeatable.
fn noise(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Scatter specks over these slices (start, end, depth). A small cleanup gets
/// a small puff and a big one a bigger puff, never more than `MAX_SPECKS`;
/// every slice gets at least one (the widest first, if there are more slices
/// than specks) and the rest go by width. Empty when there's nothing on screen
/// to puff from.
pub fn specks(slices: &[(f32, f32, usize)]) -> Vec<Speck> {
    let mut slices: Vec<(f32, f32, usize)> = slices.iter().copied().filter(|(s, e, _)| e > s).collect();
    let total: f32 = slices.iter().map(|(s, e, _)| e - s).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let budget = ((total * 200.0).round() as usize).clamp(8, MAX_SPECKS);
    slices.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
    slices.truncate(budget);
    let spare = budget - slices.len();
    let mut out = Vec::new();
    let mut seed = 1u32;
    for &(start, end, depth) in &slices {
        let width = end - start;
        let n = 1 + (width / total * spare as f32) as usize;
        for _ in 0..n {
            out.push(Speck {
                turns: start + width * noise(seed),
                across: 0.15 + 0.7 * noise(seed + 1),
                depth,
                drift: 10.0 + 22.0 * noise(seed + 2),
                size: 1.6 + 2.2 * noise(seed + 3),
            });
            seed += 4;
        }
    }
    out
}

/// Where a speck is `k` (0..1) of the way through the puff, relative to the
/// chart's centre, given its ring's radii; and its radius and opacity there.
/// It drifts outward and a little up, shrinking and fading as it goes.
pub fn speck_at(speck: &Speck, ring: (f32, f32), k: f32) -> (f32, f32, f32, f32) {
    let k = k.clamp(0.0, 1.0);
    let ease = 1.0 - (1.0 - k).powi(2);
    let radius = ring.0 + (ring.1 - ring.0) * speck.across + speck.drift * ease;
    let theta = speck.turns * std::f32::consts::TAU;
    let x = radius * theta.sin();
    let y = -radius * theta.cos() - 8.0 * ease;
    let size = speck.size * (1.0 - 0.5 * k);
    let alpha = 0.85 * (1.0 - k).powi(2);
    (x, y, size, alpha)
}

// ---------------------------------------------------------------------------
// Asides while a scan runs
// ---------------------------------------------------------------------------

/// A warm line about one of the well-known places the scan has already
/// measured. Never about the person, only about the pile.
fn fact_line(title: &str, size: u64) -> Option<String> {
    let size = format_size(size);
    Some(match title {
        "Trash" => format!("{size} already in the Trash, waiting for a nudge."),
        "Downloads" => format!("Downloads holds {size}. Installers love it there."),
        "Xcode build files" => format!("{size} of Xcode build files. Xcode makes more whenever it likes."),
        "Xcode archives" => format!("{size} of Xcode archives, every release on file."),
        "iOS device support" => format!("{size} of iOS device support, fetched again when needed."),
        "iOS simulators" => format!("{size} of simulators. A whole fleet of tiny iPhones."),
        "iPhone & iPad backups" => format!("{size} of device backups. Worth a look later."),
        "Docker" => format!("Docker is holding {size}. Images do pile up."),
        "App caches" => format!("{size} of app caches. Apps rebuild those on their own."),
        "npm cache" => format!("{size} of npm cache. Someone's been busy."),
        "Cargo registry" => format!("{size} of crates in the Cargo registry."),
        "Gradle caches" => format!("{size} of Gradle caches. Builds remember everything."),
        "Movies" => format!("{size} in Movies. Popcorn not included."),
        "Chrome update leftovers" => format!("{size} of Chrome update leftovers."),
        "Mail" => format!("{size} of Mail. Attachments add up."),
        _ => return None,
    })
}

/// How long each aside stays up.
const ASIDE_SECS: f32 = 4.6;

/// The aside for the scanning bubble at `secs` into the scan: once the hotspot
/// pass has measured something worth mentioning, its facts take turns, biggest
/// first, alternating with the file-count line when there is one.
pub fn scan_aside(early: &[findings::Early], secs: f32, has_count_line: bool) -> Option<String> {
    let mut found: Vec<&findings::Early> = early.iter().filter(|e| e.size >= findings::MIN_SIZE).collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| b.size.cmp(&a.size));
    let slot = (secs.max(0.0) / ASIDE_SECS) as usize;
    let pick = if has_count_line {
        // Odd slots belong to the facts, even ones to the file count.
        if slot % 2 == 0 {
            return None;
        }
        slot / 2
    } else {
        slot
    };
    let early = found[pick % found.len()];
    fact_line(findings::CATALOG.get(early.category)?.title, early.size)
}

// ---------------------------------------------------------------------------
// The lifetime tally
// ---------------------------------------------------------------------------

/// How much Broom has moved to the Trash, ever. Kept beside the settings but
/// in its own file: it's a record, not a choice, and the settings file stays
/// "what the user picked".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tally {
    pub bytes: u64,
    /// Moves to the Trash that went through.
    pub sweeps: u64,
}

/// Round numbers worth a word when a cleanup carries the total past them.
const MILESTONES: &[u64] = &[
    1_000_000_000,
    5_000_000_000,
    10_000_000_000,
    25_000_000_000,
    50_000_000_000,
    100_000_000_000,
    250_000_000_000,
    500_000_000_000,
    1_000_000_000_000,
    2_000_000_000_000,
    5_000_000_000_000,
    10_000_000_000_000,
];

impl Tally {
    /// Count a move to the Trash. Returns the milestone it carried the total
    /// past, if any (the biggest, when one cleanup crosses several).
    pub fn add(&mut self, bytes: u64) -> Option<u64> {
        let before = self.bytes;
        self.bytes = self.bytes.saturating_add(bytes);
        self.sweeps += 1;
        MILESTONES.iter().rev().copied().find(|&m| before < m && self.bytes >= m)
    }

    /// Things were put back from the Trash: they weren't swept after all.
    pub fn take_back(&mut self, bytes: u64) {
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    /// "Broom has swept 12.4 GB over 9 cleanups", or `None` before the first.
    pub fn summary(&self) -> Option<String> {
        if self.bytes == 0 || self.sweeps == 0 {
            return None;
        }
        let times = if self.sweeps == 1 { "1 cleanup".to_string() } else { format!("{} cleanups", crate::scan::format_count(self.sweeps)) };
        Some(format!("Broom has swept {} over {times}", format_size(self.bytes)))
    }
}

fn tally_path() -> Option<PathBuf> {
    settings::state_dir().map(|dir| dir.join("swept.json"))
}

fn load_tally() -> Tally {
    tally_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Written whole and renamed into place, like the settings, so a crash can't
/// leave half a file that reads as zero.
fn save_tally(tally: &Tally) {
    let Some(path) = tally_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string(tally) else { return };
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// The tally, read from disk once per process.
static TALLY: Mutex<Option<Tally>> = Mutex::new(None);

fn with_tally<T>(f: impl FnOnce(&mut Tally) -> T) -> T {
    let mut slot = TALLY.lock().unwrap_or_else(|e| e.into_inner());
    let tally = slot.get_or_insert_with(load_tally);
    f(tally)
}

pub fn tally() -> Tally {
    with_tally(|t| *t)
}

/// Count a move to the Trash and save; returns the milestone crossed, if any.
pub fn record_sweep(bytes: u64) -> Option<u64> {
    with_tally(|t| {
        let crossed = t.add(bytes);
        save_tally(t);
        crossed
    })
}

/// Undo part of the tally after a Put back, and save.
pub fn record_put_back(bytes: u64) {
    with_tally(|t| {
        t.take_back(bytes);
        save_tally(t);
    });
}

/// The line for a milestone, shown once, in the banner of the cleanup that
/// reached it.
pub fn milestone_line(milestone: u64) -> String {
    // Milestones are round numbers; "1.00 GB" would read like a measurement.
    let round = if milestone >= 1_000_000_000_000 {
        format!("{} TB", milestone / 1_000_000_000_000)
    } else {
        format!("{} GB", milestone / 1_000_000_000)
    };
    format!("That makes {round} swept with Broom.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_pick_a_countable_thing() {
        assert_eq!(comparison(10_000), None, "too small to compare");
        // 2.7 GB: phone photos (~900), songs (~340), minutes of 4K video (~16)
        // and CDs (~4) qualify; emails and floppies run past a thousand.
        let line = comparison(2_700_000_000).unwrap();
        assert!(["phone photos", "songs", "minutes of 4K video", "CDs"].iter().any(|w| line.ends_with(w)), "{line}");
        // The same size always reads the same.
        assert_eq!(comparison(2_700_000_000), comparison(2_700_000_000));
        // Huge cleanups still find something under a thousand: 3 TB is
        // ~750 HD movies or ~200 seasons of a show.
        let line = comparison(3_000_000_000_000).unwrap();
        assert!(line.contains("seasons of a show") || line.contains("HD movies"), "{line}");
        // Past what the biggest thing can count, there's no comparison.
        assert_eq!(comparison(20_000_000_000_000), None);
        // Just under one email still counts as one.
        assert_eq!(comparison(72_000).as_deref(), Some("about an email"));
    }

    #[test]
    fn comparison_counts_are_rounded_like_people_say_them() {
        assert_eq!(round_nice(7.4), 7);
        assert_eq!(round_nice(43.0), 45);
        assert_eq!(round_nice(437.0), 450);
        assert_eq!(round_nice(912.0), 900);
        // Every candidate a comparison names has a count below a thousand.
        for bytes in [80_000, 3_000_000, 50_000_000, 900_000_000, 12_000_000_000, 400_000_000_000] {
            let line = comparison(bytes).unwrap();
            let n: u64 = line.split(' ').nth(1).and_then(|w| w.replace(',', "").parse().ok()).unwrap_or(1);
            assert!(n < 1000, "{bytes}: {line}");
        }
    }

    #[test]
    fn single_things_read_as_one() {
        // 1.4 MB is about one floppy, the only candidate that close to one;
        // whichever is picked, a count of one is never written as "1 …s".
        for bytes in [72_000u64, 1_400_000, 3_100_000, 4_100_000_000, 15_500_000_000] {
            let line = comparison(bytes).unwrap();
            assert!(!line.starts_with("about 1 "), "{bytes}: {line}");
        }
    }

    #[test]
    fn count_up_eases_to_the_exact_target() {
        let target = 1_234_567_890;
        assert_eq!(counted(target, Duration::ZERO), 0);
        let half = counted(target, COUNT_UP / 2);
        assert!(half > target / 2 && half < target, "ease-out is past halfway at half time: {half}");
        assert_eq!(counted(target, COUNT_UP), target);
        assert_eq!(counted(target, COUNT_UP * 3), target);
        // Monotonic.
        let mut last = 0;
        for ms in 0..=600 {
            let v = counted(target, Duration::from_millis(ms));
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn counting_keeps_the_final_unit_and_width() {
        let target = 45_600_000_000; // "45.6 GB"
        assert_eq!(format_like(target, target), format_size(target));
        assert_eq!(format_like(0, target), "\u{2007}0.0 GB");
        assert_eq!(format_like(9_900_000_000, target), "\u{2007}9.9 GB");
        for v in [0, 1_000_000, 9_999_999_999, target] {
            assert_eq!(format_like(v, target).chars().count(), format_size(target).chars().count());
        }
        assert_eq!(format_like(0, 512), "\u{2007}\u{2007}0 bytes");
        assert_eq!(format_like(3_000_000, 812_000_000), "\u{2007}\u{2007}3 MB");
    }

    #[test]
    fn specks_cover_the_slices_within_budget() {
        assert!(specks(&[]).is_empty());
        assert!(specks(&[(0.3, 0.3, 1)]).is_empty(), "nothing on screen, no puff");
        let one = specks(&[(0.1, 0.2, 2)]);
        assert_eq!(one.len(), 20, "a tenth of the chart, a modest puff");
        assert!(one.iter().all(|s| (0.1..=0.2).contains(&s.turns) && s.depth == 2 && (0.0..=1.0).contains(&s.across)));
        assert_eq!(specks(&[(0.5, 0.501, 4)]).len(), 8, "a sliver still puffs");
        // A thin slice beside a wide one still gets a speck.
        let mixed = specks(&[(0.0, 0.5, 1), (0.5, 0.501, 3)]);
        assert!(mixed.len() <= MAX_SPECKS);
        assert!(mixed.iter().any(|s| s.depth == 3));
        // More slices than specks: capped, widest first.
        let many: Vec<(f32, f32, usize)> = (0..100).map(|i| (i as f32 / 100.0, (i as f32 + 0.9) / 100.0, 1 + i % 3)).collect();
        assert_eq!(specks(&many).len(), MAX_SPECKS);
        // Repeatable.
        assert_eq!(specks(&[(0.1, 0.2, 2)]), one);
    }

    #[test]
    fn specks_drift_out_and_fade() {
        let speck = Speck { turns: 0.25, across: 0.5, depth: 1, drift: 20.0, size: 3.0 };
        let (x0, _, s0, a0) = speck_at(&speck, (100.0, 140.0), 0.0);
        let (x1, y1, s1, a1) = speck_at(&speck, (100.0, 140.0), 1.0);
        // At three o'clock, outward is +x.
        assert!((x0 - 120.0).abs() < 0.01);
        assert!((x1 - 140.0).abs() < 0.01 && y1 < 0.0, "drifts out and up");
        assert!(s1 < s0 && a1 == 0.0 && a0 > 0.5);
    }

    fn early(category: &str, size: u64) -> findings::Early {
        let category = findings::CATALOG.iter().position(|c| c.title == category).unwrap();
        findings::Early { category, path: PathBuf::from("/x"), size, at_ms: 0 }
    }

    #[test]
    fn every_catalog_place_has_an_aside() {
        for category in findings::CATALOG {
            assert!(fact_line(category.title, 1_000_000_000).is_some(), "no aside for {}", category.title);
        }
    }

    #[test]
    fn scan_asides_take_turns_biggest_first() {
        assert_eq!(scan_aside(&[], 10.0, false), None);
        let small = early("npm cache", 1_000);
        assert_eq!(scan_aside(&[small], 10.0, false), None, "too small to mention");
        let found = [early("npm cache", 2_000_000_000), early("Docker", 9_000_000_000)];
        let first = scan_aside(&found, 0.0, false).unwrap();
        assert!(first.starts_with("Docker"), "{first}");
        let second = scan_aside(&found, ASIDE_SECS + 0.1, false).unwrap();
        assert!(second.contains("npm"), "{second}");
        // With a file-count line, facts only take the odd slots.
        assert_eq!(scan_aside(&found, 0.0, true), None);
        assert!(scan_aside(&found, ASIDE_SECS + 0.1, true).unwrap().starts_with("Docker"));
    }

    #[test]
    fn tally_counts_milestones_once() {
        let mut tally = Tally::default();
        assert_eq!(tally.summary(), None);
        assert_eq!(tally.add(600_000_000), None);
        assert_eq!(tally.add(600_000_000), Some(1_000_000_000));
        assert_eq!(tally.add(100_000_000), None, "already past it");
        // One big cleanup past several names the biggest.
        assert_eq!(tally.add(30_000_000_000), Some(25_000_000_000));
        assert_eq!(tally.summary().unwrap(), "Broom has swept 31.3 GB over 4 cleanups");
        tally.take_back(31_300_000_000);
        assert_eq!(tally.bytes, 0);
        tally.take_back(5);
        assert_eq!(tally.bytes, 0, "never below zero");
        assert_eq!(milestone_line(1_000_000_000), "That makes 1 GB swept with Broom.");
        assert_eq!(milestone_line(250_000_000_000), "That makes 250 GB swept with Broom.");
        assert_eq!(milestone_line(2_000_000_000_000), "That makes 2 TB swept with Broom.");
        let one = Tally { bytes: 5_000_000, sweeps: 1 };
        assert_eq!(one.summary().unwrap(), "Broom has swept 5.00 MB over 1 cleanup");
    }

    #[test]
    fn tally_survives_a_relaunch() {
        let _guard = settings::STATE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join("broom-delight-tally");
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: the lock serialises every test that touches this variable.
        unsafe { std::env::set_var("BROOM_STATE_DIR", &dir) };
        assert_eq!(load_tally(), Tally::default(), "no file reads as nothing swept");
        save_tally(&Tally { bytes: 42, sweeps: 2 });
        assert_eq!(load_tally(), Tally { bytes: 42, sweeps: 2 });
        std::fs::write(dir.join("swept.json"), "not json").unwrap();
        assert_eq!(load_tally(), Tally::default(), "a damaged file is ignored, not fatal");
        unsafe { std::env::remove_var("BROOM_STATE_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
