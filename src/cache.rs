//! The last finished scan of a root, kept on disk so the next launch (or the
//! next pick of that root) can show it at once and refresh behind it.
//!
//! One file per scanned root in `<state dir>/trees/`. A home folder is millions
//! of nodes, so the format is a small custom binary one rather than JSON:
//!
//! ```text
//! "BROOMTRE" u32 version
//! header   varints and strings (see `Meta`)
//! nodes    varint count, then in preorder: tag byte (kind), name id, size,
//!          [items, child count] for folders, parent distance
//! names    varint count, then each distinct name once (len + UTF-8)
//! trailer  u64 offset of the names, u64 node count, "BROOMEND"
//! ```
//!
//! Names repeat a lot (`node_modules`, `index.js`, `Contents`), so each is
//! stored once and nodes refer to it by number; loading then makes one string
//! per distinct name and every node shares it. Preorder with the children in
//! their sorted order means a parent always comes before its children and the
//! children come back in the order they were saved, so nothing needs sorting on
//! the way in. Only nodes still attached to the root are written, so a tree that
//! had things trashed or grafted comes back compact.
//!
//! Written to a temp file and renamed into place, so a crash leaves the old
//! file or the new one, never half of one. Anything that doesn't parse, or
//! whose trailer is missing (a truncated write), reads as "no cache".

use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::SharedString;

use crate::scan::{Kind, Node, Tree};

const MAGIC: &[u8; 8] = b"BROOMTRE";
const END: &[u8; 8] = b"BROOMEND";
/// Bumped whenever the layout changes; an older file then reads as no cache.
const VERSION: u32 = 1;
/// Trees kept at most. A home folder's is ~150 MB and this is a disk-space app,
/// so only the few most recently written stay.
const KEEP: usize = 4;
/// A scan this quick isn't worth a file: reading it again is as fast as loading it.
pub const WORTH_CACHING: Duration = Duration::from_millis(1500);

/// What a cached tree is, beyond its nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// What the user asked to scan (`/` for the startup disk, whose tree is
    /// rooted at the Data volume).
    pub requested_root: PathBuf,
    /// When the scan that produced this tree finished.
    pub taken: SystemTime,
    /// The FSEvents event id from just before that scan started reading: every
    /// change after it is either in the tree or in the event history. 0 = none.
    pub event_id: u64,
    /// The FSEvents store's id for the volume. If it changes (the volume was
    /// erased, or its event history reset) the event ids above mean nothing.
    pub volume_uuid: String,
    /// The tree root's device and inode: a root deleted and recreated, or a
    /// different disk mounted at the same path, is a different tree.
    pub root_dev: u64,
    pub root_ino: u64,
    /// The exclusions the scan honoured; a tree read with others isn't this one.
    pub excludes: u64,
}

/// A stable fingerprint of the exclusion list, order-independent.
pub fn excludes_key(excludes: &[PathBuf]) -> u64 {
    let mut lines: Vec<String> = excludes.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    lines.sort();
    lines.dedup();
    fnv(lines.join("\n").as_bytes())
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn dir() -> Option<PathBuf> {
    Some(crate::settings::state_dir()?.join("trees"))
}

/// Where the tree for `requested_root` lives.
pub fn file_for(requested_root: &Path) -> Option<PathBuf> {
    Some(dir()?.join(format!("{:016x}.tree", fnv(requested_root.as_os_str().as_encoded_bytes()))))
}

/// Save `tree` as the cache for `meta.requested_root`, then drop the oldest
/// files beyond `KEEP`. Returns the bytes written.
pub fn save(meta: &Meta, tree: &Tree) -> io::Result<u64> {
    let path = file_for(&meta.requested_root).ok_or_else(|| io::Error::other("no state directory"))?;
    let written = save_at(&path, meta, tree)?;
    prune(&path);
    Ok(written)
}

/// The cached tree for `requested_root`, if there is one and it reads cleanly.
pub fn load(requested_root: &Path) -> Option<(Meta, Tree)> {
    let (meta, tree) = load_at(&file_for(requested_root)?)?;
    (meta.requested_root == requested_root).then_some((meta, tree))
}

/// Just the header, for deciding whether loading the rest is worth it.
pub fn peek(requested_root: &Path) -> Option<Meta> {
    use std::io::Read;
    let mut head = vec![0u8; 64 << 10];
    let mut file = std::fs::File::open(file_for(requested_root)?).ok()?;
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]).ok()? {
            0 => break,
            n => filled += n,
        }
    }
    let mut reader = Reader { bytes: &head[..filled], at: 0 };
    let meta = read_header(&mut reader)?.0;
    (meta.requested_root == requested_root).then_some(meta)
}

/// Remove the cached tree for `requested_root` (it no longer describes the disk).
pub fn forget(requested_root: &Path) {
    if let Some(path) = file_for(requested_root) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn save_at(path: &Path, meta: &Meta, tree: &Tree) -> io::Result<u64> {
    let parent = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
    std::fs::create_dir_all(parent)?;
    // Unique per writer, so two saves of the same root can't share a temp file.
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    let result = write(&temp, meta, tree);
    match result {
        Ok(bytes) => {
            std::fs::rename(&temp, path)?;
            Ok(bytes)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn write(path: &Path, meta: &Meta, tree: &Tree) -> io::Result<u64> {
    let mut out = Counted { inner: BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?), written: 0 };
    out.write_all(MAGIC)?;
    out.write_all(&VERSION.to_le_bytes())?;
    let mut buf = Vec::with_capacity(4096);
    put_bytes(&mut buf, meta.requested_root.as_os_str().as_encoded_bytes());
    put_bytes(&mut buf, tree.root_path.as_os_str().as_encoded_bytes());
    put(&mut buf, meta.taken.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    put(&mut buf, meta.event_id);
    put_bytes(&mut buf, meta.volume_uuid.as_bytes());
    put(&mut buf, meta.root_dev);
    put(&mut buf, meta.root_ino);
    put(&mut buf, meta.excludes);
    put(&mut buf, tree.errors);
    put(&mut buf, tree.cloud_only);

    // Count what's attached first: the node count leads the node section.
    let mut attached = 0u64;
    let mut stack = vec![Tree::ROOT];
    while let Some(ix) = stack.pop() {
        attached += 1;
        stack.extend(tree.nodes[ix].children.iter().copied());
    }
    put(&mut buf, attached);
    out.write_all(&buf)?;

    // Preorder; each stack entry carries its parent's new index.
    let mut names: HashMap<&str, u64> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    let mut stack: Vec<(usize, u64)> = vec![(Tree::ROOT, u64::MAX)];
    let mut next = 0u64;
    while let Some((ix, parent)) = stack.pop() {
        let node = &tree.nodes[ix];
        buf.clear();
        buf.push(match node.kind {
            Kind::Dir => 0,
            Kind::File => 1,
            Kind::Other => 2,
        });
        let id = *names.entry(node.name.as_ref()).or_insert_with(|| {
            order.push(node.name.as_ref());
            order.len() as u64 - 1
        });
        put(&mut buf, id);
        put(&mut buf, node.size);
        if node.kind != Kind::File {
            put(&mut buf, node.items);
            put(&mut buf, node.children.len() as u64);
        }
        if ix != Tree::ROOT {
            put(&mut buf, next - parent);
        }
        out.write_all(&buf)?;
        // Reversed, so the first child is popped (and written) first.
        stack.extend(node.children.iter().rev().map(|&c| (c, next)));
        next += 1;
    }

    let names_at = out.written;
    buf.clear();
    put(&mut buf, order.len() as u64);
    out.write_all(&buf)?;
    for name in order {
        buf.clear();
        put_bytes(&mut buf, name.as_bytes());
        out.write_all(&buf)?;
    }
    out.write_all(&names_at.to_le_bytes())?;
    out.write_all(&attached.to_le_bytes())?;
    out.write_all(END)?;
    out.inner.flush()?;
    Ok(out.written)
}

pub fn load_at(path: &Path) -> Option<(Meta, Tree)> {
    let bytes = std::fs::read(path).ok()?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Option<(Meta, Tree)> {
    let trailer = bytes.len().checked_sub(24)?;
    if &bytes[trailer + 16..] != END {
        return None;
    }
    let names_at = u64::from_le_bytes(bytes[trailer..trailer + 8].try_into().ok()?) as usize;
    let count = u64::from_le_bytes(bytes[trailer + 8..trailer + 16].try_into().ok()?) as usize;

    let mut names_reader = Reader { bytes: bytes.get(..trailer)?, at: names_at };
    let name_count = names_reader.varint()? as usize;
    // Every name takes at least one byte, so a count beyond the bytes is damage.
    if name_count > trailer {
        return None;
    }
    let mut names: Vec<SharedString> = Vec::with_capacity(name_count);
    for _ in 0..name_count {
        let raw = names_reader.bytes()?;
        names.push(SharedString::from(std::str::from_utf8(raw).ok()?.to_owned()));
    }

    let mut reader = Reader { bytes: bytes.get(..names_at)?, at: 0 };
    let (meta, root_path, errors, cloud_only, declared) = read_header(&mut reader)?;
    // Each node takes at least three bytes.
    if declared as usize != count || count == 0 || count > names_at / 3 {
        return None;
    }
    let mut nodes: Vec<Node> = Vec::with_capacity(count);
    // How many children each folder said it has, to check against what arrived.
    let mut expected: Vec<u32> = Vec::with_capacity(count);
    for ix in 0..count {
        let kind = match reader.byte()? {
            0 => Kind::Dir,
            1 => Kind::File,
            2 => Kind::Other,
            _ => return None,
        };
        let name = names.get(reader.varint()? as usize)?.clone();
        let size = reader.varint()?;
        let (items, children) = if kind == Kind::File { (1, 0) } else { (reader.varint()?, reader.varint()? as usize) };
        let parent = if ix == 0 {
            None
        } else {
            let distance = reader.varint()? as usize;
            let parent = ix.checked_sub(distance).filter(|_| distance > 0)?;
            Some(parent)
        };
        if let Some(p) = parent {
            if nodes[p].children.len() as u32 >= expected[p] {
                return None;
            }
            nodes[p].children.push(ix);
        }
        expected.push(children.min(u32::MAX as usize) as u32);
        nodes.push(Node { name, size, kind, parent, children: Vec::with_capacity(children.min(count)), items });
    }
    if reader.at != names_at || nodes.iter().zip(&expected).any(|(n, &e)| n.children.len() as u32 != e) {
        return None;
    }
    Some((meta, Tree { root_path, nodes, errors, cloud_only, index: Arc::default() }))
}

#[allow(clippy::type_complexity)]
fn read_header(reader: &mut Reader) -> Option<(Meta, PathBuf, u64, u64, u64)> {
    if reader.take(8)? != MAGIC || u32::from_le_bytes(reader.take(4)?.try_into().ok()?) != VERSION {
        return None;
    }
    use std::os::unix::ffi::OsStrExt;
    let os = |raw: &[u8]| PathBuf::from(std::ffi::OsStr::from_bytes(raw));
    let requested_root = os(reader.bytes()?);
    let root_path = os(reader.bytes()?);
    let taken = UNIX_EPOCH + Duration::from_secs(reader.varint()?);
    let event_id = reader.varint()?;
    let volume_uuid = String::from_utf8(reader.bytes()?.to_vec()).ok()?;
    let root_dev = reader.varint()?;
    let root_ino = reader.varint()?;
    let excludes = reader.varint()?;
    let errors = reader.varint()?;
    let cloud_only = reader.varint()?;
    let count = reader.varint()?;
    Some((Meta { requested_root, taken, event_id, volume_uuid, root_dev, root_ino, excludes }, root_path, errors, cloud_only, count))
}

/// Keep the `KEEP` most recently written trees; `keep` (just written) always stays.
fn prune(keep: &Path) {
    let Some(parent) = keep.parent() else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "tree") && p != keep)
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, old) in files.into_iter().skip(KEEP - 1) {
        let _ = std::fs::remove_file(old);
    }
}

/// LEB128: sizes, counts and indices are mostly small, so most take a byte or two.
fn put(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    put(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            value |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.varint()? as usize;
        self.take(n)
    }
}

/// Counts what goes through, for the names offset in the trailer.
struct Counted<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for Counted<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let node = |name: &str, size: u64, kind: Kind, parent: Option<usize>, children: Vec<usize>, items: u64| Node {
            name: name.to_string().into(),
            size,
            kind,
            parent,
            children,
            items,
        };
        Tree {
            root_path: PathBuf::from("/x"),
            nodes: vec![
                node("x", 300, Kind::Dir, None, vec![1, 3, 4], 2),
                node("a", 200, Kind::Dir, Some(0), vec![2], 1),
                node("f.bin", 200, Kind::File, Some(1), vec![], 1),
                node("g", 99, Kind::File, Some(0), vec![], 1),
                node("Not readable", 1, Kind::Other, Some(0), vec![], 0),
                // Detached (trashed): must not be written.
                node("gone", 5, Kind::File, None, vec![], 1),
            ],
            errors: 3,
            cloud_only: 1,
            index: Arc::default(),
        }
    }

    fn meta() -> Meta {
        Meta {
            requested_root: PathBuf::from("/x"),
            taken: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            event_id: 123_456_789_012,
            volume_uuid: "ABC-DEF".into(),
            root_dev: 16_777_234,
            root_ino: 2,
            excludes: excludes_key(&[PathBuf::from("/x/skip")]),
        }
    }

    /// Damage of every kind reads as no cache, never as a wrong tree or a panic.
    #[test]
    fn damaged_files_read_as_nothing() {
        let dir = std::env::temp_dir().join(format!("broom-cache-damage-{}", std::process::id()));
        let path = dir.join("t.tree");
        save_at(&path, &meta(), &tree()).unwrap();
        let good = std::fs::read(&path).unwrap();
        assert!(parse(&good).is_some());
        // Every truncation, and a flipped byte everywhere.
        for len in 0..good.len() {
            assert!(parse(&good[..len]).is_none(), "truncated to {len}");
        }
        for at in 0..good.len() {
            let mut bad = good.clone();
            bad[at] ^= 0xff;
            // A flipped size byte can still parse (it's just a number); what matters
            // is no panic, and a structurally broken file never loads.
            let _ = parse(&bad);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_ignore_order() {
        let a = [PathBuf::from("/a"), PathBuf::from("/b")];
        let b = [PathBuf::from("/b"), PathBuf::from("/a")];
        assert_eq!(excludes_key(&a), excludes_key(&b));
        assert_ne!(excludes_key(&a), excludes_key(&[]));
    }

    /// Detached nodes are dropped and the rest keep their shape.
    #[test]
    fn writes_only_what_is_attached() {
        let dir = std::env::temp_dir().join(format!("broom-cache-attached-{}", std::process::id()));
        let path = dir.join("t.tree");
        save_at(&path, &meta(), &tree()).unwrap();
        let (m, loaded) = load_at(&path).unwrap();
        assert_eq!(m, meta());
        assert_eq!(loaded.nodes.len(), 5);
        assert!(loaded.nodes.iter().all(|n| n.name.as_ref() != "gone"));
        assert_eq!((loaded.errors, loaded.cloud_only), (3, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
