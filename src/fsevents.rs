//! What changed on disk since a moment in the past, from the FSEvents history.
//!
//! macOS logs every change to every volume (that hasn't opted out) as "this
//! folder's contents changed", each with a system-wide, ever-increasing event
//! id. Note the id before a scan starts reading, and later a stream opened
//! with `sinceWhen` = that id replays every folder touched in between, then
//! says `HistoryDone`. Re-reading just those folders brings the old tree up to
//! date in a fraction of a full walk (see `scan::catchup`).
//!
//! The replay is trusted only while it can be: if the history was lost or
//! coalesced (`MustScanSubDirs` on the root, dropped events, wrapped ids), the
//! root itself moved, or the volume's event store is not the one the id came
//! from, the answer is "scan everything", never a guess.
//!
//! Raw CoreServices calls: the crate has no FSEvents binding, and these few
//! functions are all it needs.

use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::scan::Change;

type CFRef = *const c_void;

#[repr(C)]
struct StreamContext {
    version: isize,
    info: *mut c_void,
    retain: CFRef,
    release: CFRef,
    copy_description: CFRef,
}

type Callback = extern "C" fn(stream: CFRef, info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32, ids: *const u64);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(dev: libc::dev_t) -> CFRef;
    fn FSEventStreamCreate(allocator: CFRef, callback: Callback, context: *const StreamContext, paths: CFRef, since: u64, latency: f64, flags: u32) -> CFRef;
    fn FSEventStreamSetDispatchQueue(stream: CFRef, queue: *mut c_void);
    fn FSEventStreamStart(stream: CFRef) -> u8;
    fn FSEventStreamStop(stream: CFRef);
    fn FSEventStreamInvalidate(stream: CFRef);
    fn FSEventStreamRelease(stream: CFRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFStringCreateWithBytes(allocator: CFRef, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFRef;
    fn CFArrayCreate(allocator: CFRef, values: *const CFRef, count: isize, callbacks: *const c_void) -> CFRef;
    fn CFUUIDCreateString(allocator: CFRef, uuid: CFRef) -> CFRef;
    fn CFStringGetCString(string: CFRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: CFRef);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_release(object: *mut c_void);
}

const UTF8: u32 = 0x0800_0100;
const MUST_SCAN_SUB_DIRS: u32 = 0x1;
const USER_DROPPED: u32 = 0x2;
const KERNEL_DROPPED: u32 = 0x4;
const EVENT_IDS_WRAPPED: u32 = 0x8;
const HISTORY_DONE: u32 = 0x10;
const ROOT_CHANGED: u32 = 0x20;
/// Folder-level events, as the stream is documented to give by default.
const CREATE_FLAG_NONE: u32 = 0;

/// The newest event id: note it before reading, and every later change is in
/// the history after it.
pub fn current_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

/// The FSEvents store's id for this device, or `None` when the volume keeps
/// no history (a disk image mounted with `nobrowse`, a network share, a
/// volume with `.fseventsd/no_log`): there's nothing to replay there.
pub fn volume_uuid(dev: u64) -> Option<String> {
    unsafe {
        let uuid = FSEventsCopyUUIDForDevice(dev as libc::dev_t);
        if uuid.is_null() {
            return None;
        }
        let string = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        if string.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 64];
        let ok = CFStringGetCString(string, buf.as_mut_ptr(), buf.len() as isize, UTF8);
        CFRelease(string);
        (ok != 0).then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// Why the history can't be used and everything must be read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// FSEvents coalesced or dropped events at or above the root.
    History,
    /// The event ids wrapped around.
    Wrapped,
    /// The root itself was moved, deleted or replaced.
    RootChanged,
    /// The stream couldn't be created or started.
    NoStream,
    /// The replay didn't finish in time.
    Timeout,
}

impl Lost {
    pub fn reason(self) -> &'static str {
        match self {
            Lost::History => "FSEvents lost track of changes under the root",
            Lost::Wrapped => "FSEvents ids wrapped",
            Lost::RootChanged => "the root itself changed",
            Lost::NoStream => "no FSEvents stream",
            Lost::Timeout => "the FSEvents replay timed out",
        }
    }
}

#[derive(Default)]
struct State {
    changes: Vec<Change>,
    lost: Option<Lost>,
    done: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Events are reported against the real path (`/private/var/…`); the tree
    /// knows the folder by the path it was given (`/var/…`).
    real_root: PathBuf,
}

extern "C" fn on_events(_stream: CFRef, info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32, _ids: *const u64) {
    // SAFETY: `info` is the `Shared` that `changes_since` keeps alive until the
    // stream is invalidated and its queue drained; FSEvents hands `count` C
    // strings and flags.
    let shared = unsafe { &*(info as *const Shared) };
    let paths = paths as *const *const c_char;
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    for i in 0..count {
        let (flag, path) = unsafe { (*flags.add(i), CStr::from_ptr(*paths.add(i))) };
        if flag & HISTORY_DONE != 0 {
            state.done = true;
            continue;
        }
        if state.done {
            // Live events after the replay: the next catch-up gets them.
            continue;
        }
        if flag & EVENT_IDS_WRAPPED != 0 {
            state.lost = Some(Lost::Wrapped);
            continue;
        }
        if flag & ROOT_CHANGED != 0 {
            state.lost = Some(Lost::RootChanged);
            continue;
        }
        let path = PathBuf::from(path.to_string_lossy().trim_end_matches('/'));
        let path = if path.as_os_str().is_empty() { PathBuf::from("/") } else { path };
        let recursive = flag & (MUST_SCAN_SUB_DIRS | USER_DROPPED | KERNEL_DROPPED) != 0;
        // Lost track at the root or above it: nothing below can be trusted.
        if recursive && shared.real_root.starts_with(&path) {
            if std::env::var_os("BROOM_TIMING").is_some() {
                eprintln!("fsevents: lost track at {} (flags {flag:#x})", path.display());
            }
            state.lost = Some(Lost::History);
            continue;
        }
        state.changes.push(Change { path, recursive });
    }
    drop(state);
    shared.wake.notify_all();
}

extern "C" fn nothing(_: *mut c_void) {}

/// Every folder under `root` whose contents changed after event `since`,
/// as FSEvents recorded them. Paths come back under `root` as given, even
/// when it reaches its folder through a symlink.
pub fn changes_since(root: &Path, since: u64, timeout: Duration) -> Result<Vec<Change>, Lost> {
    use std::os::unix::ffi::OsStrExt;
    let real_root = std::fs::canonicalize(root).map_err(|_| Lost::RootChanged)?;
    let shared = Box::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), real_root: real_root.clone() });
    let context = StreamContext {
        version: 0,
        info: &*shared as *const Shared as *mut c_void,
        retain: std::ptr::null(),
        release: std::ptr::null(),
        copy_description: std::ptr::null(),
    };
    let collected = unsafe {
        let bytes = real_root.as_os_str().as_bytes();
        let path = CFStringCreateWithBytes(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, UTF8, 0);
        if path.is_null() {
            return Err(Lost::NoStream);
        }
        let paths = CFArrayCreate(std::ptr::null(), &path, 1, &raw const kCFTypeArrayCallBacks);
        CFRelease(path);
        if paths.is_null() {
            return Err(Lost::NoStream);
        }
        let stream = FSEventStreamCreate(std::ptr::null(), on_events, &context, paths, since, 0.05, CREATE_FLAG_NONE);
        CFRelease(paths);
        if stream.is_null() {
            return Err(Lost::NoStream);
        }
        let queue = dispatch_queue_create(c"broom.fsevents".as_ptr(), std::ptr::null());
        FSEventStreamSetDispatchQueue(stream, queue);
        let started = FSEventStreamStart(stream) != 0;
        let outcome = if started {
            let deadline = Instant::now() + timeout;
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while !state.done && state.lost.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = shared.wake.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
            }
            match (state.lost, state.done) {
                (Some(lost), _) => Err(lost),
                (None, true) => Ok(std::mem::take(&mut state.changes)),
                (None, false) => Err(Lost::Timeout),
            }
        } else {
            Err(Lost::NoStream)
        };
        if started {
            FSEventStreamStop(stream);
        }
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
        // Wait out a callback already running on the queue before `shared` goes.
        dispatch_sync_f(queue, std::ptr::null_mut(), nothing);
        dispatch_release(queue);
        outcome
    };
    drop(shared);
    let mut changes = collected?;
    // Back to the root's own spelling. Anything that doesn't map is a path this
    // code doesn't understand; guessing it away could hide a change, so give up.
    for change in &mut changes {
        change.path = map_back(&change.path, &real_root, root).ok_or(Lost::History)?;
    }
    Ok(changes)
}

/// An event path as the tree spells it: under `root` rather than its real
/// path. The Data volume's folders also show up through their firmlinks
/// (`/Users/…` for `/System/Volumes/Data/Users/…`).
fn map_back(path: &Path, real_root: &Path, root: &Path) -> Option<PathBuf> {
    const DATA: &str = "/System/Volumes/Data";
    if let Ok(rest) = path.strip_prefix(real_root) {
        return Some(root.join(rest));
    }
    if real_root.starts_with(DATA) && !path.starts_with(DATA) {
        let through = Path::new(DATA).join(path.strip_prefix("/").ok()?);
        return map_back(&through, real_root, root);
    }
    if !real_root.starts_with(DATA) {
        if let Ok(rest) = path.strip_prefix(DATA) {
            return map_back(&Path::new("/").join(rest), real_root, root);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_event_paths_to_the_roots_spelling() {
        let (real, root) = (Path::new("/private/var/t"), Path::new("/var/t"));
        assert_eq!(map_back(Path::new("/private/var/t/a/b"), real, root), Some(PathBuf::from("/var/t/a/b")));
        assert_eq!(map_back(Path::new("/private/var/t"), real, root), Some(PathBuf::from("/var/t")));
        assert_eq!(map_back(Path::new("/elsewhere"), real, root), None);
        let data = Path::new("/System/Volumes/Data");
        assert_eq!(map_back(Path::new("/Users/me/x"), data, data), Some(PathBuf::from("/System/Volumes/Data/Users/me/x")));
        assert_eq!(map_back(Path::new("/System/Volumes/Data/Users"), data, data), Some(PathBuf::from("/System/Volumes/Data/Users")));
        let home = Path::new("/Users/me");
        assert_eq!(map_back(Path::new("/System/Volumes/Data/Users/me/x"), home, home), Some(PathBuf::from("/Users/me/x")));
        assert_eq!(map_back(Path::new("/System/Volumes/Data/Users/you"), home, home), None);
    }
}
