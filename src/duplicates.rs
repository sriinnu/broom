//! Duplicate files: the same bytes stored twice, where deleting the extra copy
//! really frees space. Runs after a scan, in the background.
//!
//! Only the user's own files are considered: nothing inside an app bundle,
//! a hidden folder, a Library, node_modules or the system's folders, where a
//! "duplicate" is part of something that needs it. Candidates are grouped by
//! size, then by their first 64 KB, then compared byte for byte. Hard links
//! (one file, two names) are counted once, and an APFS clone costs only the
//! bytes it doesn't share, so a pure clone frees nothing and is left out.
//!
//! An extra copy can go to the Trash, or be merged: replaced in place by an
//! APFS clone of the copy that stays (`merge`). Both files stay where they
//! are, with their own names, dates, permissions and tags, but the bytes are
//! stored once; editing either later takes new space only for what changes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use crate::dirlist;
use crate::findings::{self, Finding, Safety};
use crate::scan::{Kind, Tree};

/// Smaller files aren't worth reading twice.
const MIN_FILE: u64 = 1 << 20;
const HEAD: usize = 64 * 1024;
/// Folders whose contents belong to something else.
const SKIP: &[&str] = &["Library", "node_modules", "System", "Applications", "private", "Volumes", "usr", "bin", "sbin", "opt", "cores"];

/// One extra copy and the copy that stays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub extra: PathBuf,
    pub keep: PathBuf,
    /// What removing (or merging) the extra frees.
    pub frees: u64,
}

/// The duplicates under `tree`, as one finding, if together they waste enough
/// to be worth a look, with which copy each extra duplicates.
pub fn find(tree: &Tree) -> Option<(Finding, Vec<Pair>)> {
    find_min(tree, MIN_FILE, findings::MIN_SIZE)
}

fn find_min(tree: &Tree, min_file: u64, min_total: u64) -> Option<(Finding, Vec<Pair>)> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut stack = vec![Tree::ROOT];
    while let Some(ix) = stack.pop() {
        for &c in &tree.nodes[ix].children {
            let node = &tree.nodes[c];
            match node.kind {
                Kind::Dir if !skipped(&node.name) => stack.push(c),
                Kind::File if node.size >= min_file => by_size.entry(node.size).or_default().push(c),
                _ => {}
            }
        }
    }

    let mut extras = Vec::new();
    let mut pairs = Vec::new();
    let mut wasted = 0;
    for nodes in by_size.into_values().filter(|n| n.len() > 1) {
        let files: Vec<(usize, PathBuf)> = unique_files(nodes.into_iter().map(|ix| (ix, tree.path_of(ix))));
        for mut class in identical(files) {
            // Keep the copy nearest the top (then alphabetically); the rest are extra.
            class.sort_by(|a, b| (a.1.components().count(), &a.1).cmp(&(b.1.components().count(), &b.1)));
            let keep = class[0].1.clone();
            for (ix, path) in class.into_iter().skip(1) {
                let frees = frees(&path, tree.nodes[ix].size);
                if frees >= min_file {
                    extras.push(ix);
                    wasted += frees;
                    pairs.push(Pair { extra: path, keep: keep.clone(), frees });
                }
            }
        }
    }
    (wasted >= min_total && !extras.is_empty()).then(|| (Finding {
        id: findings::next_id(),
        title: "Duplicate files",
        blurb: format!(
            "{} extra {}; one of each stays",
            extras.len(),
            if extras.len() == 1 { "copy" } else { "copies" },
        ),
        safety: Safety::Review,
        size: wasted,
        path: None,
        nodes: extras,
        pending: false,
    }, pairs))
}

/// Why a copy was left as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// It changed (or went) since the scan: the two aren't the same any more.
    Changed,
    /// Not on the same APFS volume as the copy that stays, so no clone.
    NoClone,
    /// Locked, or hard-linked under another name (merging would keep the
    /// other name's bytes and free nothing).
    Locked,
    Failed(String),
}

const CLONE_NOFOLLOW: u32 = 0x0001;

fn c_path(path: &Path) -> Result<std::ffi::CString, Skip> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| Skip::Failed("path has a NUL".into()))
}

fn os_error(what: &str) -> Skip {
    Skip::Failed(format!("{what}: {}", std::io::Error::last_os_error()))
}

/// Replace `pair.extra` with an APFS clone of `pair.keep`, keeping the extra's
/// own name, place, permissions, dates (created and modified), flags, ACL and
/// extended attributes (Finder tags, "where from"). The swap is one rename:
/// at every moment the path holds either the old file or the finished clone.
/// Returns the bytes freed (what the extra didn't share before).
pub fn merge(pair: &Pair) -> Result<u64, Skip> {
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::fs::MetadataExt;
    let (extra, keep) = (&pair.extra, &pair.keep);
    let meta = extra.symlink_metadata().map_err(|_| Skip::Changed)?;
    let kept = keep.symlink_metadata().map_err(|_| Skip::Changed)?;
    if !meta.is_file() || !kept.is_file() || meta.len() != kept.len() {
        return Err(Skip::Changed);
    }
    let immutable = (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | libc::UF_APPEND | libc::SF_APPEND) as u32;
    if meta.nlink() > 1 || meta.st_flags() & immutable != 0 {
        return Err(Skip::Locked);
    }
    if meta.dev() != kept.dev() {
        return Err(Skip::NoClone);
    }
    // Already one data stream: nothing to do, nothing freed.
    if file_sharing_private(extra) == Some(0) {
        return Ok(0);
    }
    let before = frees(extra, meta.len());
    if !same_bytes(keep, extra) {
        return Err(Skip::Changed);
    }
    let name = extra.file_name().ok_or(Skip::Changed)?.to_string_lossy();
    let tmp = extra.with_file_name(format!(".{name}.broom-clone-{}", std::process::id()));
    let (src, dst, from) = (c_path(keep)?, c_path(&tmp)?, c_path(extra)?);
    let _ = std::fs::remove_file(&tmp);
    if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), CLONE_NOFOLLOW) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ENOTSUP) | Some(libc::EXDEV) => Skip::NoClone,
            _ => Skip::Failed(format!("clone: {error}")),
        });
    }
    let finish = || -> Result<(), Skip> {
        // The clone carries the kept copy's attributes; it should carry the extra's.
        for attr in xattr_names(&dst) {
            unsafe { libc::removexattr(dst.as_ptr(), attr.as_ptr(), libc::XATTR_NOFOLLOW) };
        }
        if unsafe { libc::copyfile(from.as_ptr(), dst.as_ptr(), std::ptr::null_mut(), libc::COPYFILE_METADATA) } != 0 {
            return Err(os_error("copy attributes"));
        }
        set_created(&dst, meta.st_birthtime(), meta.st_birthtime_nsec())?;
        // Last look before the swap: the extra must still be the file we read.
        let now = extra.symlink_metadata().map_err(|_| Skip::Changed)?;
        if now.ino() != meta.ino() || now.mtime() != meta.mtime() || now.mtime_nsec() != meta.mtime_nsec() || now.len() != meta.len() {
            return Err(Skip::Changed);
        }
        std::fs::rename(&tmp, extra).map_err(|e| Skip::Failed(format!("swap: {e}")))
    };
    match finish() {
        Ok(()) => Ok(before),
        Err(skip) => {
            let _ = std::fs::remove_file(&tmp);
            Err(skip)
        }
    }
}

fn file_sharing_private(path: &Path) -> Option<u64> {
    dirlist::file_sharing(path).filter(|s| s.private != dirlist::PRIVATE_UNKNOWN).map(|s| s.private)
}

fn xattr_names(path: &std::ffi::CStr) -> Vec<std::ffi::CString> {
    let size = unsafe { libc::listxattr(path.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if size <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size as usize];
    let size = unsafe { libc::listxattr(path.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), libc::XATTR_NOFOLLOW) };
    if size <= 0 {
        return Vec::new();
    }
    buf[..size as usize].split(|&b| b == 0).filter(|n| !n.is_empty()).filter_map(|n| std::ffi::CString::new(n).ok()).collect()
}

/// Set a file's creation date (Finder's "Created"), which a clone starts
/// fresh and `copyfile` doesn't carry.
fn set_created(path: &std::ffi::CStr, secs: i64, nanos: i64) -> Result<(), Skip> {
    let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.commonattr = libc::ATTR_CMN_CRTIME;
    let mut when = libc::timespec { tv_sec: secs, tv_nsec: nanos };
    let set = unsafe {
        libc::setattrlist(
            path.as_ptr(),
            (&mut attrs as *mut libc::attrlist).cast(),
            (&mut when as *mut libc::timespec).cast(),
            std::mem::size_of::<libc::timespec>(),
            libc::FSOPT_NOFOLLOW as _,
        )
    };
    if set != 0 { Err(os_error("set created date")) } else { Ok(()) }
}

fn skipped(name: &str) -> bool {
    name.starts_with('.') || Tree::is_bundled_name(name) || SKIP.contains(&name)
}

/// One entry per file on disk: a hard link to a file already listed is dropped.
fn unique_files(files: impl Iterator<Item = (usize, PathBuf)>) -> Vec<(usize, PathBuf)> {
    use std::os::unix::fs::MetadataExt;
    let mut seen = std::collections::HashSet::new();
    files
        .filter(|(_, path)| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && seen.insert((m.dev(), m.ino()))))
        .collect()
}

/// Split same-sized files into classes of identical content (classes of one dropped).
fn identical(files: Vec<(usize, PathBuf)>) -> Vec<Vec<(usize, PathBuf)>> {
    let mut by_head: HashMap<Vec<u8>, Vec<(usize, PathBuf)>> = HashMap::new();
    for file in files {
        if let Some(head) = read_head(&file.1) {
            by_head.entry(head).or_default().push(file);
        }
    }
    let mut classes: Vec<Vec<(usize, PathBuf)>> = Vec::new();
    for bucket in by_head.into_values().filter(|b| b.len() > 1) {
        let mut found: Vec<Vec<(usize, PathBuf)>> = Vec::new();
        for file in bucket {
            match found.iter_mut().find(|class| same_bytes(&class[0].1, &file.1)) {
                Some(class) => class.push(file),
                None => found.push(vec![file]),
            }
        }
        classes.extend(found.into_iter().filter(|c| c.len() > 1));
    }
    classes
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD);
    File::open(path).ok()?.take(HEAD as u64).read_to_end(&mut head).ok()?;
    Some(head)
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(fa), Ok(fb)) = (File::open(a), File::open(b)) else { return false };
    let (mut ra, mut rb) = (BufReader::with_capacity(1 << 20, fa), BufReader::with_capacity(1 << 20, fb));
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let (Ok(na), Ok(nb)) = (read_full(&mut ra, &mut ba), read_full(&mut rb, &mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// Fill `buf` as far as the file goes.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// What deleting this copy frees: all of it, or for an APFS clone only the
/// blocks it shares with nothing else.
fn frees(path: &Path, size: u64) -> u64 {
    match dirlist::file_sharing(path) {
        Some(sharing) if sharing.private == dirlist::PRIVATE_UNKNOWN => dirlist::private_size(path).unwrap_or(size),
        Some(sharing) => sharing.private,
        None => size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_found_and_the_shallowest_kept() {
        let dir = std::env::temp_dir().join(format!("broom-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/deeper")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        let one: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut other = one.clone();
        *other.last_mut().unwrap() ^= 1; // same size and head, different tail
        std::fs::write(dir.join("keep.bin"), &one).unwrap();
        std::fs::write(dir.join("a/deeper/copy.bin"), &one).unwrap();
        std::fs::write(dir.join("a/near-miss.bin"), &other).unwrap();
        std::fs::write(dir.join(".hidden/copy.bin"), &one).unwrap();
        std::fs::hard_link(dir.join("keep.bin"), dir.join("a/link.bin")).unwrap();

        let tree = crate::scan::scan(&dir, &crate::scan::Progress::default(), &[], 2);
        let (finding, pairs) = find_min(&tree, 1, 1).expect("a duplicate");
        assert_eq!(pairs, vec![Pair { extra: dir.join("a/deeper/copy.bin"), keep: dir.join("keep.bin"), frees: pairs[0].frees }]);
        let paths: Vec<PathBuf> = finding.nodes.iter().map(|&ix| tree.path_of(ix)).collect();
        assert_eq!(paths, vec![dir.join("a/deeper/copy.bin")], "{paths:?}");
        assert_eq!(finding.safety, Safety::Review);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn set_xattr(path: &Path, name: &str, value: &[u8]) {
        let (p, n) = (c_path(path).unwrap(), std::ffi::CString::new(name).unwrap());
        assert_eq!(unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) }, 0);
    }

    fn get_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
        let (p, n) = (c_path(path).unwrap(), std::ffi::CString::new(name).unwrap());
        let mut buf = vec![0u8; 256];
        let got = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
        (got >= 0).then(|| buf[..got as usize].to_vec())
    }

    #[test]
    fn merging_makes_a_clone_that_keeps_the_copys_own_name_dates_and_tags() {
        use std::os::macos::fs::MetadataExt as _;
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("broom-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let bytes: Vec<u8> = (0..2_000_000u32).map(|i| (i * 13 % 251) as u8).collect();
        let (keep, extra) = (dir.join("keep.bin"), dir.join("sub/extra.bin"));
        std::fs::write(&keep, &bytes).unwrap();
        std::fs::write(&extra, &bytes).unwrap();
        set_xattr(&keep, "com.example.keep", b"k");
        set_xattr(&extra, "com.example.tag", b"red");
        std::fs::set_permissions(&extra, std::fs::Permissions::from_mode(0o640)).unwrap();
        // Give the extra its own, older dates.
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::options().write(true).open(&extra).unwrap().set_modified(old).unwrap();
        let before = extra.metadata().unwrap();

        let pair = Pair { extra: extra.clone(), keep: keep.clone(), frees: 0 };
        let freed = merge(&pair).expect("merged");
        assert!(freed >= 1_900_000, "freed {freed}");
        let after = extra.metadata().unwrap();
        assert_eq!(std::fs::read(&extra).unwrap(), bytes);
        assert_eq!(after.permissions().mode() & 0o777, 0o640);
        assert_eq!(after.modified().unwrap(), old);
        assert_eq!(after.st_birthtime(), before.st_birthtime());
        assert_eq!(get_xattr(&extra, "com.example.tag").as_deref(), Some(&b"red"[..]));
        assert_eq!(get_xattr(&extra, "com.example.keep"), None, "the kept copy's tags stay on it");
        assert_eq!(file_sharing_private(&extra), Some(0), "now a pure clone");
        assert!(std::fs::read_dir(dir.join("sub")).unwrap().count() == 1, "no temp file left");
        // Merging again does nothing and frees nothing.
        assert_eq!(merge(&pair), Ok(0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merging_leaves_changed_linked_and_locked_copies_alone() {
        let dir = std::env::temp_dir().join(format!("broom-merge-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = vec![5u8; 300_000];
        let keep = dir.join("keep.bin");
        std::fs::write(&keep, &bytes).unwrap();
        let pair = |name: &str| Pair { extra: dir.join(name), keep: keep.clone(), frees: 0 };

        let mut changed = bytes.clone();
        changed[299_999] = 6;
        std::fs::write(dir.join("changed.bin"), &changed).unwrap();
        assert_eq!(merge(&pair("changed.bin")), Err(Skip::Changed));
        assert_eq!(std::fs::read(dir.join("changed.bin")).unwrap(), changed);

        std::fs::write(dir.join("linked.bin"), &bytes).unwrap();
        std::fs::hard_link(dir.join("linked.bin"), dir.join("other-name.bin")).unwrap();
        assert_eq!(merge(&pair("linked.bin")), Err(Skip::Locked));

        std::fs::write(dir.join("locked.bin"), &bytes).unwrap();
        let locked = c_path(&dir.join("locked.bin")).unwrap();
        assert_eq!(unsafe { libc::chflags(locked.as_ptr(), libc::UF_IMMUTABLE as _) }, 0);
        assert_eq!(merge(&pair("locked.bin")), Err(Skip::Locked));
        unsafe { libc::chflags(locked.as_ptr(), 0) };

        assert_eq!(merge(&pair("gone.bin")), Err(Skip::Changed));
        let _ = std::fs::remove_dir_all(dir);
    }
}
