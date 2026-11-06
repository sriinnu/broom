//! The protect list: files and folders Broom must never offer or move to the
//! Trash. They're still scanned and shown (unlike an exclude); they just never
//! appear in a finding, can't be staged, and the move to the Trash refuses
//! them. "Them" includes anything holding them: trashing `~/Projects` would
//! take a protected `~/Projects/keep` with it, so that's refused too.
//!
//! The guard that matters is in `trashcan::send`, the one door every move to
//! the Trash goes through (the app, the menu-bar panel and the CLI agents
//! drive). Everything else here is so the UI never offers what that door
//! would refuse, and says so when it leaves something out.

use std::path::{Path, PathBuf};

/// Whether moving `path` to the Trash would move `protected` with it, or is
/// `protected` itself, or something inside it.
pub fn touches(path: &Path, protected: &Path) -> bool {
    path.starts_with(protected) || protected.starts_with(path)
}

/// The first protected path `path` touches. Compared as given and with both
/// sides resolved, so `/tmp/x` and `/private/tmp/x` are the same thing.
pub fn blocking<'a>(path: &Path, list: &'a [PathBuf]) -> Option<&'a PathBuf> {
    how(path, list).map(|(p, _)| p)
}

/// `blocking`, and whether `path` only holds the protected thing (it's
/// strictly above it) rather than being it or inside it.
pub fn how<'a>(path: &Path, list: &'a [PathBuf]) -> Option<(&'a PathBuf, bool)> {
    if list.is_empty() {
        return None;
    }
    let holds = |a: &Path, b: &Path| b.starts_with(a) && a != b;
    if let Some(p) = list.iter().find(|p| touches(path, p)) {
        return Some((p, holds(path, p)));
    }
    let real = resolved(path)?;
    list.iter().find_map(|p| {
        let b = resolved(p)?;
        touches(&real, &b).then(|| (p, holds(&real, &b)))
    })
}

/// `path` with symlinks resolved; for a path that doesn't exist (yet, or any
/// more), its parent's resolution joined to the name.
fn resolved(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))
}

/// The list as saved in Settings, read fresh: an edit in another window, or
/// by hand, counts at once.
pub fn current() -> Vec<PathBuf> {
    crate::settings::load().settings.protect_paths()
}

/// Split `paths` into what may go and what may not, with the protected path
/// each refused one touches.
/// `Refused.2` is true when the path only holds the protected one.
pub fn sort_out(paths: &[PathBuf], list: &[PathBuf]) -> (Vec<PathBuf>, Vec<Refused>) {
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    for path in paths {
        match how(path, list) {
            Some((p, holds)) => refused.push((path.clone(), p.clone(), holds)),
            None => ok.push(path.clone()),
        }
    }
    (ok, refused)
}

/// A path that may not go, the protected path it touches, and whether it
/// only holds it.
pub type Refused = (PathBuf, PathBuf, bool);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_itself_and_above_are_all_protected() {
        let list = vec![PathBuf::from("/Users/a/Projects/keep")];
        assert!(blocking(Path::new("/Users/a/Projects/keep"), &list).is_some());
        assert!(blocking(Path::new("/Users/a/Projects/keep/src/main.rs"), &list).is_some());
        assert!(blocking(Path::new("/Users/a/Projects"), &list).is_some(), "trashing the parent takes it along");
        assert!(blocking(Path::new("/Users/a/Projects/keeper"), &list).is_none(), "a sibling with a longer name");
        assert!(blocking(Path::new("/Users/a/Projects/other"), &list).is_none());
        assert!(blocking(Path::new("/anything"), &[]).is_none());
    }

    #[test]
    fn symlinked_spellings_count() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("broom-protect-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("real/keep")).unwrap();
        let link = dir.join("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(dir.join("real"), &link).unwrap();
        let list = vec![link.join("keep")];
        assert!(blocking(&dir.join("real/keep"), &list).is_some());
        assert!(blocking(&dir.join("real"), &list).is_some());
        assert_eq!(how(&dir.join("real/keep"), &list).map(|(_, holds)| holds), Some(false), "itself, by another spelling");
        assert_eq!(how(&dir.join("real"), &list).map(|(_, holds)| holds), Some(true));
        let (ok, refused) = sort_out(&[dir.join("real/other"), dir.join("real/keep/x")], &list);
        assert_eq!(ok, vec![dir.join("real/other")]);
        assert_eq!(refused.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
