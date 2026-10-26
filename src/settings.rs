//! Where Broom keeps what the user has chosen.
//!
//! One JSON file in Application Support, written whole and written atomically:
//! a rename either happened or it didn't, so a crash mid-save can't leave a
//! half-file that the next launch reads as "the user turned everything off".
//!
//! The schema is versioned and every field is `#[serde(default)]`. A file from
//! an older Broom is missing fields, not malformed, and a file from a newer one
//! carries fields this build doesn't know — both must load. Only a file that
//! isn't JSON at all is treated as damage, and even then it is moved aside
//! rather than deleted: it might be the only record of what someone set up.
//!
//! Nothing here touches the UI thread. Reads happen once at startup, writes go
//! out whole.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::{Appearance, ThemeName};

/// Bumped only when a change can't be expressed as a new defaulted field.
pub const SCHEMA: u32 = 1;

/// The startup disk: what a fresh install scans if the user hasn't said.
pub const DEFAULT_ROOT: &str = "/";

/// Where Broom keeps its small bits of state. `BROOM_STATE_DIR` overrides it.
/// Shared with `onboarding`, so the first-run marker lands in the same folder.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BROOM_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Broom"))
}

pub fn file_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("settings.json"))
}

/// How the chart is coloured. Mirrors `app::ColorBy`, which owns the painting;
/// this is only the on-disk spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorBy {
    /// Each slice its own hue around the circle.
    #[default]
    Folder,
    /// By what things are: apps, caches, photos…
    Kind,
}

/// Everything the user can change, and the only thing that gets written.
///
/// Defaults are what Broom did before this file existed — the constants that
/// used to be compiled in. Someone who never opens Settings gets the same app
/// they had yesterday.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version, so a future build can migrate rather than guess.
    pub v: u32,
    /// Folders to leave out of the scan, as typed. Expanded and resolved at
    /// scan time, not here — `~` means whatever the user's home is then.
    pub excludes: Vec<String>,
    /// Levels named up front before the main walk, so the chart can label them
    /// at once. Higher fills the ring faster and costs more on huge trees.
    pub outline_depth: usize,
    /// Milliseconds between mid-scan chart refreshes. Lower is smoother and
    /// busier; this is the one setting with a real cost either way.
    pub live_refresh_ms: u64,
    /// Seconds the "freed" banner stays after a deletion.
    pub banner_seconds: u64,
    /// Keep the colouring across a rescan and across launches.
    pub color_by: ColorBy,
    /// Show the 🧹 extra in the menu bar.
    pub menubar: bool,
    /// How Broom looks, and whether it follows macOS between light and dark.
    pub theme: ThemeName,
    pub appearance: Appearance,
    /// Scan this when Broom opens with no folder argument.
    pub default_root: String,
    /// The last folder actually scanned, so a relaunch lands where you were.
    /// Written by Broom, not shown in Settings.
    pub last_root: Option<String>,
    /// Column widths in points, as last dragged. Written by Broom, not shown
    /// in Settings; double-clicking a column edge puts one back.
    pub rail_width: f32,
    pub navigator_width: f32,
    pub cleanup_width: f32,
}

/// Default column widths, and how far a drag may take each.
pub const RAIL_WIDTH: f32 = 200.0;
pub const NAVIGATOR_WIDTH: f32 = 300.0;
pub const CLEANUP_WIDTH: f32 = 300.0;
pub const RAIL_RANGE: (f32, f32) = (150.0, 360.0);
pub const NAVIGATOR_RANGE: (f32, f32) = (220.0, 680.0);
pub const CLEANUP_RANGE: (f32, f32) = (240.0, 560.0);

impl Default for Settings {
    fn default() -> Self {
        Self {
            v: SCHEMA,
            excludes: Vec::new(),
            outline_depth: 2,
            live_refresh_ms: 100,
            banner_seconds: 8,
            color_by: ColorBy::Folder,
            menubar: true,
            theme: ThemeName::Classic,
            appearance: Appearance::Auto,
            default_root: DEFAULT_ROOT.to_string(),
            last_root: None,
            rail_width: RAIL_WIDTH,
            navigator_width: NAVIGATOR_WIDTH,
            cleanup_width: CLEANUP_WIDTH,
        }
    }
}

impl Settings {
    /// Clamp anything a hand-edited file could get wrong.
    ///
    /// A depth of 40 or a refresh of 0 ms wouldn't corrupt anything, but they
    /// would make the app look broken, and "the file says 0" is a worse error
    /// message than silently doing the sane thing.
    fn sanitized(mut self) -> Self {
        self.v = SCHEMA;
        self.outline_depth = self.outline_depth.clamp(0, 6);
        self.live_refresh_ms = self.live_refresh_ms.clamp(16, 2000);
        self.banner_seconds = self.banner_seconds.clamp(2, 120);
        // Keep the list short — this is a list of folders, not a corpus.
        self.excludes.truncate(64);
        let fit = |w: f32, default: f32, (lo, hi): (f32, f32)| if w.is_finite() { w.clamp(lo, hi) } else { default };
        self.rail_width = fit(self.rail_width, RAIL_WIDTH, RAIL_RANGE);
        self.navigator_width = fit(self.navigator_width, NAVIGATOR_WIDTH, NAVIGATOR_RANGE);
        self.cleanup_width = fit(self.cleanup_width, CLEANUP_WIDTH, CLEANUP_RANGE);
        self.excludes.retain(|line| !line.trim().is_empty());
        if self.default_root.trim().is_empty() {
            self.default_root = DEFAULT_ROOT.to_string();
        }
        self
    }

    /// Where a scan should start when the user hasn't named a folder: the last
    /// one, else their preferred default.
    pub fn startup_root(&self) -> PathBuf {
        self.last_root
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| PathBuf::from(&self.default_root))
    }

    /// Excluded folders as paths, with `~` expanded against the current home.
    /// Entries that don't exist are kept: the user may be excluding a folder
    /// on a volume that isn't mounted yet.
    pub fn exclude_paths(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        self.excludes
            .iter()
            .map(|line| match (line.strip_prefix("~/"), &home) {
                (Some(rest), Some(home)) => home.join(rest),
                _ => PathBuf::from(line),
            })
            .collect()
    }
}

/// What loading produced, including whether anything had to be salvaged.
/// The caller decides whether to tell the user; this layer doesn't know how.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub settings: Settings,
    /// Set when the file existed but couldn't be understood, and was moved to
    /// this path. The UI surfaces it once; the file is never deleted.
    pub salvaged: Option<PathBuf>,
}

/// Read settings from disk, falling back to defaults for any reason at all.
///
/// A missing file is the normal first-run case and is not noteworthy. A file
/// that won't parse is moved to `settings.json.bad` so the next save doesn't
/// silently discard whatever it held.
pub fn load() -> Loaded {
    let Some(path) = file_path() else {
        return Loaded { settings: Settings::default(), salvaged: None };
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Loaded { settings: Settings::default(), salvaged: None };
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(settings) => Loaded { settings: settings.sanitized(), salvaged: None },
        Err(_) => {
            let bad = path.with_extension("json.bad");
            let salvaged = std::fs::rename(&path, &bad).ok().map(|()| bad);
            Loaded { settings: Settings::default(), salvaged }
        }
    }
}

/// Write settings, replacing the file in one step.
///
/// Sibling temp file, then rename: same directory, so the rename is atomic
/// rather than a copy across filesystems.
pub fn save(settings: &Settings) -> Result<(), String> {
    let Some(path) = file_path() else { return Err("no home directory".into()) };
    let Some(dir) = path.parent() else { return Err("no settings directory".into()) };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

/// Held by every test that points `BROOM_STATE_DIR` somewhere: the variable
/// is process-wide and the tests run in parallel.
#[cfg(test)]
pub(crate) static STATE_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `BROOM_STATE_DIR` at a fresh folder for the duration of a test.
    /// These tests share the process environment, so they run under one lock.
    fn with_temp_dir<T>(name: &str, body: impl FnOnce() -> T) -> T {
        let _guard = STATE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!("broom-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: the lock above serialises every test that reads this var.
        unsafe { std::env::set_var("BROOM_STATE_DIR", &dir) };
        let out = body();
        unsafe { std::env::remove_var("BROOM_STATE_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn roundtrips_every_field() {
        with_temp_dir("roundtrip", || {
            let mut settings = Settings::default();
            settings.excludes = vec!["~/Movies".into(), "/Volumes/Backup".into()];
            settings.outline_depth = 3;
            settings.live_refresh_ms = 250;
            settings.banner_seconds = 4;
            settings.color_by = ColorBy::Kind;
            settings.menubar = false;
            settings.default_root = "/Users".into();
            settings.last_root = Some("/Users/someone/Code".into());
            save(&settings).unwrap();

            let loaded = load();
            assert_eq!(loaded.settings, settings);
            assert!(loaded.salvaged.is_none());
        });
    }

    #[test]
    fn missing_file_is_defaults_not_an_error() {
        with_temp_dir("missing", || {
            let loaded = load();
            assert_eq!(loaded.settings, Settings::default());
            assert!(loaded.salvaged.is_none());
        });
    }

    /// A file written by an older Broom is missing fields, not malformed.
    #[test]
    fn partial_file_fills_in_the_rest() {
        with_temp_dir("partial", || {
            let path = file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{"v":1,"menubar":false}"#).unwrap();

            let loaded = load();
            assert!(!loaded.settings.menubar);
            assert!(loaded.salvaged.is_none());
            // Everything else came from the defaults.
            assert_eq!(loaded.settings.outline_depth, Settings::default().outline_depth);
            assert_eq!(loaded.settings.color_by, ColorBy::Folder);
        });
    }

    /// Unknown fields from a newer build must not stop the file loading.
    #[test]
    fn unknown_fields_are_ignored() {
        with_temp_dir("unknown", || {
            let path = file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{"v":9,"something_new":true,"excludes":["/tmp"]}"#).unwrap();

            let loaded = load();
            assert_eq!(loaded.settings.excludes, vec!["/tmp".to_string()]);
            assert!(loaded.salvaged.is_none());
        });
    }

    /// Damage is moved aside, never deleted, and the app still starts.
    #[test]
    fn corrupt_file_is_kept_as_bad() {
        with_temp_dir("corrupt", || {
            let path = file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{ this is not json").unwrap();

            let loaded = load();
            assert_eq!(loaded.settings, Settings::default());
            let bad = loaded.salvaged.expect("damaged file should be moved aside");
            assert!(bad.exists());
            assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ this is not json");
            assert!(!path.exists(), "the damaged file should no longer be in the way");
        });
    }

    /// A hand-edited file can hold numbers that would make the app look broken.
    #[test]
    fn absurd_values_are_clamped() {
        with_temp_dir("clamp", || {
            let path = file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"outline_depth":900,"live_refresh_ms":0,"banner_seconds":99999,"default_root":"  "}"#,
            )
            .unwrap();

            let s = load().settings;
            assert_eq!(s.outline_depth, 6);
            assert_eq!(s.live_refresh_ms, 16);
            assert_eq!(s.banner_seconds, 120);
            assert_eq!(s.default_root, DEFAULT_ROOT);
        });
    }

    #[test]
    fn excludes_expand_home_and_keep_the_rest_verbatim() {
        let home = std::env::var("HOME").expect("HOME is set on macOS");
        let mut settings = Settings::default();
        settings.excludes = vec!["~/Movies".into(), "/Volumes/Backup".into()];
        let paths = settings.exclude_paths();
        assert_eq!(paths[0], PathBuf::from(&home).join("Movies"));
        assert_eq!(paths[1], PathBuf::from("/Volumes/Backup"));
    }

    /// A blank entry is a typo, not a request to exclude the whole disk.
    #[test]
    fn empty_excludes_are_dropped() {
        with_temp_dir("empty-excludes", || {
            let path = file_path().unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{"excludes":["","   ","/tmp"]}"#).unwrap();
            assert_eq!(load().settings.excludes, vec!["/tmp".to_string()]);
        });
    }

    #[test]
    fn startup_root_prefers_a_real_last_folder() {
        let mut settings = Settings::default();
        settings.default_root = "/nonexistent-preference".into();
        // No last root: the preference is used as-is, even if it's gone.
        assert_eq!(settings.startup_root(), PathBuf::from("/nonexistent-preference"));

        // A last root that exists wins.
        settings.last_root = Some(std::env::temp_dir().to_string_lossy().into_owned());
        assert_eq!(settings.startup_root(), std::env::temp_dir());

        // A last root that has since been deleted falls back to the preference.
        settings.last_root = Some("/nonexistent-last".into());
        assert_eq!(settings.startup_root(), PathBuf::from("/nonexistent-preference"));
    }
}