//! Duplicate files: the same bytes stored twice, where deleting the extra copy
//! really frees space. Runs after a scan, in the background.
//!
//! Only the user's own files are considered: nothing inside an app bundle,
//! a hidden folder, a Library, node_modules or the system's folders, where a
//! "duplicate" is part of something that needs it. Candidates are grouped by
//! size, then by their first 64 KB, then compared byte for byte. Hard links
//! (one file, two names) are counted once, and an APFS clone costs only the
//! bytes it doesn't share, so a pure clone frees nothing and is left out.

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

/// The duplicates under `tree`, as one finding, if together they waste enough
/// to be worth a look.
pub fn find(tree: &Tree) -> Option<Finding> {
    find_min(tree, MIN_FILE, findings::MIN_SIZE)
}

fn find_min(tree: &Tree, min_file: u64, min_total: u64) -> Option<Finding> {
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
    let mut wasted = 0;
    for nodes in by_size.into_values().filter(|n| n.len() > 1) {
        let files: Vec<(usize, PathBuf)> = unique_files(nodes.into_iter().map(|ix| (ix, tree.path_of(ix))));
        for mut class in identical(files) {
            // Keep the copy nearest the top (then alphabetically); the rest are extra.
            class.sort_by(|a, b| (a.1.components().count(), &a.1).cmp(&(b.1.components().count(), &b.1)));
            for (ix, path) in class.into_iter().skip(1) {
                let frees = frees(&path, tree.nodes[ix].size);
                if frees >= min_file {
                    extras.push(ix);
                    wasted += frees;
                }
            }
        }
    }
    (wasted >= min_total && !extras.is_empty()).then(|| Finding {
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
    })
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
        let finding = find_min(&tree, 1, 1).expect("a duplicate");
        let paths: Vec<PathBuf> = finding.nodes.iter().map(|&ix| tree.path_of(ix)).collect();
        assert_eq!(paths, vec![dir.join("a/deeper/copy.bin")], "{paths:?}");
        assert_eq!(finding.safety, Safety::Review);
        let _ = std::fs::remove_dir_all(dir);
    }
}
