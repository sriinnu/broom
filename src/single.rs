//! One Broom at a time. The running instance listens on a Unix socket in its
//! state folder; a second launch connects, hands over the folder it was asked
//! to scan (if any), and exits, and the first one comes forward. A stale socket
//! from a crash is swept aside. Dev runs that drive a window by script
//! (`BROOM_SNAPSHOT`, `BROOM_VIDEO`) and `BROOM_NO_SINGLETON=1` stay separate.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::settings;

fn socket_path() -> Option<PathBuf> {
    let dir = settings::state_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("broom.sock"))
}

fn exempt() -> bool {
    ["BROOM_SNAPSHOT", "BROOM_VIDEO", "BROOM_NO_SINGLETON"].iter().any(|v| std::env::var_os(v).is_some())
}

/// Returns true when this process is the Broom: it now owns the socket and
/// serves later launches. False when another Broom is already running: it has
/// been told to come forward (and to scan `initial`), and this process should exit.
pub fn claim(initial: Option<&Path>) -> bool {
    if exempt() {
        return true;
    }
    let Some(socket) = socket_path() else { return true };
    if let Ok(mut stream) = UnixStream::connect(&socket) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let line = match initial {
            Some(path) => format!("open {}\n", path.display()),
            None => "open\n".to_string(),
        };
        if stream.write_all(line.as_bytes()).is_ok() {
            let mut reply = String::new();
            if BufReader::new(&stream).read_line(&mut reply).is_ok() && reply.trim() == "ok" {
                return false;
            }
        }
        // Connected but no answer: a hung instance. Take over.
    }
    let _ = std::fs::remove_file(&socket);
    let Ok(listener) = UnixListener::bind(&socket) else { return true };
    std::thread::Builder::new()
        .name("broom-single".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.trim().strip_prefix("open").map(str::trim).filter(|p| !p.is_empty()).map(PathBuf::from);
                crate::app::open_path_requested(path);
                let mut stream = stream;
                let _ = stream.write_all(b"ok\n");
            }
        })
        .ok();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second claim in a row finds the first and yields to it.
    #[test]
    fn second_claim_yields() {
        let _guard = settings::STATE_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Not temp_dir(): macOS caps a socket path at 104 bytes and /var/folders/… is near it.
        let dir = PathBuf::from(format!("/tmp/broom-single-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: the lock above serialises every test that sets these variables.
        unsafe {
            std::env::set_var("BROOM_STATE_DIR", &dir);
            std::env::remove_var("BROOM_SNAPSHOT");
        }
        assert!(claim(None), "first launch owns the socket");
        assert!(!claim(Some(Path::new("/tmp/somewhere"))), "second launch yields");
        assert!(!claim(None), "and so does a third");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
