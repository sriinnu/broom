//! Where the disk's space is: the arithmetic, shown.
//!
//! Finder, About This Mac and `df` give three different numbers for the same
//! disk, and "System Data" swallows whatever nothing else explains. This
//! module lays the startup disk out as a ledger whose lines add up, and says
//! for every line how it was found:
//!
//! - **measured**: read from APFS or the scan just now;
//! - **computed**: arithmetic on measured lines, shown as such;
//! - **macOS's estimate**: a number only macOS knows (purgeable space), which
//!   Broom reports but can't check;
//! - **unknown**: there, but not measurable without admin rights (what a
//!   snapshot holds), and said so rather than guessed.
//!
//! Snapshots are listed too, with what each one is. Time Machine's local
//! snapshots can be deleted (by a person, with an admin password); macOS's
//! own update snapshots are named and left alone.

use std::path::Path;

use serde::Deserialize;

use crate::disk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Measured,
    Computed,
    Estimate,
    Unknown,
}

impl How {
    pub fn label(self) -> &'static str {
        match self {
            How::Measured => "measured",
            How::Computed => "computed",
            How::Estimate => "macOS's estimate",
            How::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub label: String,
    pub bytes: Option<u64>,
    pub how: How,
    /// One sentence on where the number comes from or what's in it.
    pub note: String,
    /// 0 for a total, 1 for a part of the one above (the parts add up),
    /// 2 for a detail of a part (not part of any sum).
    pub depth: u8,
}

fn line(depth: u8, label: impl Into<String>, bytes: Option<u64>, how: How, note: impl Into<String>) -> Line {
    Line { label: label.into(), bytes, how, note: note.into(), depth }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotKind {
    /// Time Machine's local snapshot, with its date as `tmutil` spells it
    /// (`2026-11-06-101500`). Deletable, with an admin password.
    TimeMachine { date: String },
    /// macOS's own: software update and the sealed system. Never touched.
    MacOs,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub kind: SnapshotKind,
    /// macOS may delete it by itself when space runs low.
    pub purgeable: bool,
    /// On the Data volume (where your files are) rather than the system one.
    pub on_data: bool,
}

#[derive(Deserialize)]
struct Listing {
    #[serde(rename = "Snapshots", default)]
    snapshots: Vec<RawSnapshot>,
}

#[derive(Deserialize)]
struct RawSnapshot {
    #[serde(rename = "SnapshotName")]
    name: String,
    #[serde(rename = "Purgeable", default)]
    purgeable: bool,
}

/// `com.apple.TimeMachine.2026-11-06-101500.local` → its date, if it is one.
fn time_machine_date(name: &str) -> Option<String> {
    let date = name.strip_prefix("com.apple.TimeMachine.")?.trim_end_matches(".local");
    // YYYY-MM-DD-HHMMSS, digits and dashes only: it ends up in a command line.
    let shape = date.len() == 17 && date.chars().enumerate().all(|(i, c)| if [4, 7, 10].contains(&i) { c == '-' } else { c.is_ascii_digit() });
    shape.then(|| date.to_string())
}

/// `diskutil apfs listSnapshots -plist`, converted to JSON, into snapshots.
pub fn parse_snapshots(json: &str, on_data: bool) -> Vec<Snapshot> {
    let Ok(listing) = serde_json::from_str::<Listing>(json) else { return Vec::new() };
    listing
        .snapshots
        .into_iter()
        .map(|raw| {
            let kind = match time_machine_date(&raw.name) {
                Some(date) => SnapshotKind::TimeMachine { date },
                None if raw.name.starts_with("com.apple.os.update") || raw.name.starts_with("com.apple.bless") => SnapshotKind::MacOs,
                None => SnapshotKind::Other,
            };
            Snapshot { name: raw.name, kind, purgeable: raw.purgeable, on_data }
        })
        .collect()
}

/// Snapshots on the volume mounted at `mount`, read-only and without admin.
fn snapshots_of(mount: &Path, on_data: bool) -> Vec<Snapshot> {
    let Ok(listed) = std::process::Command::new("/usr/sbin/diskutil").args(["apfs", "listSnapshots", "-plist"]).arg(mount).output() else {
        return Vec::new();
    };
    let Ok(mut convert) = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    else {
        return Vec::new();
    };
    use std::io::Write as _;
    if let Some(mut stdin) = convert.stdin.take() {
        let _ = stdin.write_all(&listed.stdout);
    }
    let Ok(out) = convert.wait_with_output() else { return Vec::new() };
    parse_snapshots(&String::from_utf8_lossy(&out.stdout), on_data)
}

/// Everything the ledger needs, measured now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub disk_name: String,
    pub capacity: u64,
    pub container_used: u64,
    /// What a write can use this second.
    pub free: u64,
    /// macOS's word for what it would free on demand.
    pub purgeable: u64,
    pub data_used: u64,
    /// The other volumes: macOS, swap, Preboot, updates…
    pub extras: Vec<(String, u64)>,
    /// Bytes in the files the last whole-disk scan read, if there is one.
    pub scanned: Option<u64>,
    /// Folders that scan couldn't read.
    pub unreadable: u64,
    pub snapshots: Vec<Snapshot>,
}

/// Measure the startup disk. `scanned` and `unreadable` come from the tree
/// on screen when it's a whole-disk scan. Runs `diskutil` twice: call it off
/// the UI thread.
pub fn gather(disk_name: String, scanned: Option<u64>, unreadable: u64) -> Option<Inputs> {
    let layout = disk::startup_layout(disk_name.clone())?;
    let mut root: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c"/".as_ptr(), &mut root) } != 0 {
        return None;
    }
    let block = root.f_bsize as u64;
    let purgeable = crate::scan::volumes().first().map(|v| v.purgeable).unwrap_or(0);
    let mut snapshots = snapshots_of(&layout.data_root, true);
    snapshots.extend(snapshots_of(Path::new("/"), false));
    Some(Inputs {
        disk_name,
        capacity: root.f_blocks * block,
        container_used: layout.container_used,
        free: root.f_bavail * block,
        purgeable,
        data_used: layout.data_used,
        extras: layout.extras,
        scanned,
        unreadable,
        snapshots,
    })
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// The ledger: lines that add up, each saying how it was found.
pub fn ledger(i: &Inputs) -> Vec<Line> {
    let mut lines = vec![
        line(0, format!("{} holds", i.disk_name), Some(i.capacity), How::Measured, "The APFS container's size."),
        line(0, "Used", Some(i.container_used), How::Measured, "What APFS reports as in use across every volume on the disk."),
    ];
    let tm = i.snapshots.iter().filter(|s| s.on_data && matches!(s.kind, SnapshotKind::TimeMachine { .. })).count() as u64;
    match i.scanned {
        Some(scanned) => {
            // The Data volume is the measured truth; what the scan read is a
            // detail under it (depth 2, not summed), because the two differ
            // for two honest reasons, in opposite directions:
            // - a file-by-file count includes a block shared by APFS clones
            //   (or a file and a snapshot) once per file, so it can add up to
            //   MORE than the volume holds;
            // - folders Broom couldn't read, snapshots and APFS's bookkeeping
            //   take space no file count sees, so it can add up to LESS.
            // Only the net shows. Which way it leans is shown, not hidden.
            lines.push(line(1, "Your files, apps and the rest", Some(i.data_used), How::Measured, "The Data volume: everything that isn't macOS itself."));
            lines.push(line(2, "Files Broom read", Some(scanned), How::Measured, "The last whole-disk scan, each file at its full size on disk."));
            let mut parts = Vec::new();
            if i.unreadable > 0 {
                parts.push(format!("{} Broom couldn't read (Full Disk Access lets it)", plural(i.unreadable, "folder", "folders")));
            }
            if tm > 0 {
                parts.push(format!("{} Time Machine {} on this volume", tm, if tm == 1 { "snapshot" } else { "snapshots" }));
            }
            parts.push("APFS's own bookkeeping".into());
            let unseen = parts.join(", ");
            if scanned > i.data_used {
                lines.push(line(
                    2,
                    "Counted more than once",
                    Some(scanned - i.data_used),
                    How::Computed,
                    format!(
                        "The files add up to more than the volume holds: blocks several files share (APFS clones, copies that cost nothing until edited; hard links) count once per file. At least this much is shared, more if anything is unseen: {unseen}."
                    ),
                ));
            } else {
                lines.push(line(
                    2,
                    "Not explained by files",
                    Some(i.data_used - scanned),
                    How::Computed,
                    format!("The volume minus the files read. Made of: {unseen}, less whatever clones share. APFS won't say how it splits without admin rights, so Broom doesn't guess."),
                ));
            }
            lines.push(line(2, "Since the scan", None, How::Unknown, "Files added or removed since the scan shift both numbers; scan again for a fresh count."));
        }
        None => {
            lines.push(line(1, "Your files, apps and the rest", Some(i.data_used), How::Measured, "The Data volume. Scan the whole disk to see how much of this is files Broom can read."));
        }
    }
    for (name, bytes) in &i.extras {
        let note = match name.as_str() {
            "macOS" => "The sealed system volume, read-only; includes macOS's own update snapshots.",
            "Swap (VM)" => "Memory paged to disk and the sleep image. macOS manages it.",
            "Preboot" => "What the Mac needs before macOS starts.",
            "Software updates" => "A macOS update being prepared.",
            "Recovery" => "macOS Recovery, for repairing the Mac or reinstalling macOS.",
            "Other volumes" => "What's left of the container: volumes that aren't mounted and APFS's own metadata.",
            _ => "Another volume on the same disk.",
        };
        // `disk::startup_layout` works this one out as a remainder.
        let how = if name == "Other volumes" { How::Computed } else { How::Measured };
        lines.push(line(1, name.clone(), Some(*bytes), how, note));
    }
    lines.push(line(0, "Free right now", Some(i.free), How::Measured, "What a new file can use this second."));
    // Free + used vs the size: APFS keeps a few blocks back from ordinary
    // writes. Shown, not rounded away.
    let reserved = i.capacity.saturating_sub(i.container_used + i.free);
    if reserved > 0 {
        lines.push(line(0, "Held back by APFS", Some(reserved), How::Computed, "Size minus used minus free: blocks APFS reserves and won't give to ordinary writes."));
    }
    lines.push(line(
        0,
        "Purgeable",
        Some(i.purgeable),
        How::Estimate,
        "Space macOS says it can free on demand: Time Machine snapshots, caches, iCloud files it can download again. Broom can't check this number; it isn't always right.",
    ));
    lines.push(line(0, "Finder's \u{201c}available\u{201d}", Some(i.free + i.purgeable), How::Computed, "Free plus purgeable. That's why Finder shows more room than there is right now."));
    lines
}

/// Delete these local Time Machine snapshots. macOS shows its own
/// administrator prompt; Broom never handles the password. Every date is
/// checked again to be `YYYY-MM-DD-HHMMSS` before it goes near a command.
pub fn delete_time_machine_snapshots(dates: &[String]) -> Result<(), String> {
    if dates.is_empty() {
        return Ok(());
    }
    if let Some(bad) = dates.iter().find(|d| time_machine_date(&format!("com.apple.TimeMachine.{d}.local")).as_deref() != Some(d.as_str())) {
        return Err(format!("{bad:?} isn't a snapshot date"));
    }
    let script = dates.iter().map(|d| format!("/usr/bin/tmutil deletelocalsnapshots {d}")).collect::<Vec<_>>().join(" && ");
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &format!("do shell script \"{script}\" with administrator privileges")])
        .output()
        .map_err(|e| format!("couldn't ask macOS: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let why = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // -128 is the person pressing Cancel on the password prompt.
        Err(if why.contains("-128") { "the password prompt was cancelled".into() } else { why })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> Inputs {
        Inputs {
            disk_name: "Macintosh HD".into(),
            capacity: 1_000,
            container_used: 700,
            free: 290,
            purgeable: 40,
            data_used: 600,
            extras: vec![("macOS".into(), 80), ("Swap (VM)".into(), 20)],
            scanned: Some(450),
            unreadable: 3,
            snapshots: vec![Snapshot { name: "com.apple.TimeMachine.2026-11-06-101500.local".into(), kind: SnapshotKind::TimeMachine { date: "2026-11-06-101500".into() }, purgeable: true, on_data: true }],
        }
    }

    fn find<'a>(lines: &'a [Line], label: &str) -> &'a Line {
        lines.iter().find(|l| l.label.starts_with(label)).unwrap_or_else(|| panic!("no line {label}"))
    }

    #[test]
    fn the_parts_add_up_to_used_and_the_totals_to_the_disk() {
        let i = inputs();
        let lines = ledger(&i);
        let parts: u64 = lines.iter().filter(|l| l.depth == 1).filter_map(|l| l.bytes).sum();
        assert_eq!(parts, i.container_used);
        let gap = find(&lines, "Not explained");
        assert_eq!((gap.bytes, gap.how, gap.depth), (Some(150), How::Computed, 2));
        assert_eq!(find(&lines, "Files Broom read").bytes, Some(450));
        assert!(gap.note.contains("3 folders Broom couldn't read") && gap.note.contains("1 Time Machine snapshot"));
        // Used + free + held back = the disk, to the byte.
        let top = |l: &str| find(&lines, l).bytes.unwrap();
        assert_eq!(top("Used") + top("Free") + top("Held back"), i.capacity);
        assert_eq!(top("Finder"), 330);
        assert_eq!(find(&lines, "Purgeable").how, How::Estimate);
    }

    #[test]
    fn without_a_whole_disk_scan_it_says_so_instead_of_guessing() {
        let lines = ledger(&Inputs { scanned: None, ..inputs() });
        assert!(lines.iter().all(|l| !l.label.starts_with("Not explained") && l.depth < 2));
        let data = find(&lines, "Your files, apps and the rest");
        assert_eq!(data.bytes, Some(600));
        let parts: u64 = lines.iter().filter(|l| l.depth == 1).filter_map(|l| l.bytes).sum();
        assert_eq!(parts, 700);
    }

    #[test]
    fn files_counting_more_than_the_volume_is_called_sharing_not_hidden() {
        // The real case: 993 GB of files on an 875 GB volume.
        let lines = ledger(&Inputs { scanned: Some(650), ..inputs() });
        assert!(lines.iter().all(|l| !l.label.starts_with("Not explained")));
        let shared = find(&lines, "Counted more than once");
        assert_eq!((shared.bytes, shared.how, shared.depth), (Some(50), How::Computed, 2));
        assert!(shared.note.contains("clones"));
        assert!(!lines.iter().any(|l| l.note.contains("have been removed since")), "no invented cause");
        let parts: u64 = lines.iter().filter(|l| l.depth == 1).filter_map(|l| l.bytes).sum();
        assert_eq!(parts, 700, "still adds up");
    }

    #[test]
    fn snapshots_are_told_apart_and_only_well_formed_dates_pass() {
        let json = r#"{"Snapshots":[
            {"SnapshotName":"com.apple.TimeMachine.2026-11-06-101500.local","Purgeable":true},
            {"SnapshotName":"com.apple.os.update-MSUPrepareUpdate","Purgeable":false},
            {"SnapshotName":"com.apple.TimeMachine.2026-11-06-10150; rm -rf ~.local","Purgeable":true},
            {"SnapshotName":"something.else"}]}"#;
        let s = parse_snapshots(json, true);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].kind, SnapshotKind::TimeMachine { date: "2026-11-06-101500".into() });
        assert!(s[0].purgeable && s[0].on_data);
        assert_eq!(s[1].kind, SnapshotKind::MacOs);
        assert_eq!(s[2].kind, SnapshotKind::Other, "a malformed date is never treated as one");
        assert_eq!(s[3].kind, SnapshotKind::Other);
        assert!(parse_snapshots("not json", false).is_empty());
    }

    #[test]
    fn deleting_refuses_anything_but_a_snapshot_date() {
        // Refused before any command runs, so no prompt appears in the test.
        assert!(delete_time_machine_snapshots(&["2026-11-06-101500; rm -rf ~".into()]).is_err());
        assert!(delete_time_machine_snapshots(&["2026-11-06".into()]).is_err());
        assert_eq!(delete_time_machine_snapshots(&[]), Ok(()));
    }

    /// On this Mac, the real thing: the ledger's parts add up to what APFS
    /// says is used.
    #[test]
    fn the_real_disk_adds_up() {
        let Some(i) = gather("Macintosh HD".into(), None, 0) else { return };
        let lines = ledger(&i);
        let parts: u64 = lines.iter().filter(|l| l.depth == 1).filter_map(|l| l.bytes).sum();
        assert_eq!(parts, i.container_used);
        assert!(i.capacity >= i.container_used + i.free);
    }

    /// `cargo test --release space::tests::print_ledger -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_ledger() {
        let i = gather("Macintosh HD".into(), None, 0).expect("startup disk");
        for l in ledger(&i) {
            eprintln!("{}{:<28} {:>12} [{}]", "  ".repeat(l.depth as usize), l.label, l.bytes.map(crate::scan::format_size).unwrap_or_default(), l.how.label());
        }
        for s in &i.snapshots {
            eprintln!("snapshot {:?} purgeable={} on_data={} {}", s.kind, s.purgeable, s.on_data, s.name);
        }
    }
}
