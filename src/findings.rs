//! Findings: well-known places where disk space piles up, with exact sizes and a plain
//! explanation of whether they're safe to delete. The hotspot pass scans these first, so
//! their sizes are exact within seconds of starting a scan.

use std::path::{Path, PathBuf};

use crate::scan::{Kind, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Rebuilt or re-downloaded automatically when needed.
    Safe,
    /// Worth a look first: it may hold things you want to keep.
    Review,
    /// Shown and sized, never staged: the Trash is the wrong tool (Docker's
    /// disk image, read-only module caches, toolchains a manager tracks).
    /// The blurb names the right one.
    Manage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Home,
    /// The per-user temporary folder under /var/folders.
    UserTemp,
}

pub struct Category {
    pub base: Base,
    /// Location relative to `base`.
    pub path: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub safety: Safety,
}

/// In hotspot order: most often large and most valuable first.
pub const CATALOG: &[Category] = &[
    Category { base: Base::Home, path: ".Trash", title: "Trash", blurb: "Emptying the Trash frees this space", safety: Safety::Safe },
    Category { base: Base::Home, path: "Downloads", title: "Downloads", blurb: "Old installers and archives tend to pile up here", safety: Safety::Review },
    Category { base: Base::Home, path: "Library/Developer/Xcode/DerivedData", title: "Xcode build files", blurb: "Xcode rebuilds these when you next build", safety: Safety::Safe },
    Category { base: Base::Home, path: "Library/Developer/Xcode/Archives", title: "Xcode archives", blurb: "Keep the ones you need to symbolicate crash logs", safety: Safety::Review },
    Category { base: Base::Home, path: "Library/Developer/Xcode/iOS DeviceSupport", title: "iOS device support", blurb: "Re-downloaded when a device next connects", safety: Safety::Safe },
    Category { base: Base::Home, path: "Library/Developer/CoreSimulator/Devices", title: "iOS simulators", blurb: "Remove old ones with `xcrun simctl delete unavailable`, so Xcode knows they went", safety: Safety::Manage },
    Category { base: Base::Home, path: "Library/Application Support/MobileSync/Backup", title: "iPhone & iPad backups", blurb: "Old device backups; manage them in Finder", safety: Safety::Review },
    Category { base: Base::Home, path: "Library/Containers/com.docker.docker", title: "Docker", blurb: "Images and volumes live in one disk image; prune them in Docker (`docker system prune`)", safety: Safety::Manage },
    Category { base: Base::Home, path: "Library/Caches", title: "App caches", blurb: "Apps rebuild their caches as needed", safety: Safety::Safe },
    Category { base: Base::Home, path: ".npm", title: "npm cache", blurb: "Re-downloaded on the next install", safety: Safety::Safe },
    Category { base: Base::Home, path: ".cargo/registry", title: "Cargo registry", blurb: "Re-downloaded on the next build", safety: Safety::Safe },
    Category { base: Base::Home, path: ".gradle/caches", title: "Gradle caches", blurb: "Re-downloaded on the next build", safety: Safety::Safe },
    Category { base: Base::Home, path: ".gradle/wrapper/dists", title: "Gradle versions", blurb: "Downloaded again when a project asks for one", safety: Safety::Safe },
    Category { base: Base::Home, path: ".m2/repository", title: "Maven repository", blurb: "Re-downloaded on the next build", safety: Safety::Safe },
    Category { base: Base::Home, path: ".bun/install/cache", title: "Bun cache", blurb: "Re-downloaded on the next install", safety: Safety::Safe },
    Category { base: Base::Home, path: ".cocoapods/repos", title: "CocoaPods specs", blurb: "Fetched again by `pod install`", safety: Safety::Safe },
    Category {
        base: Base::Home,
        path: "Library/pnpm/store",
        title: "pnpm store",
        blurb: "Projects' node_modules link into it, so it frees less than its size; the next install downloads again",
        safety: Safety::Review,
    },
    Category { base: Base::Home, path: "go/pkg/mod", title: "Go modules", blurb: "Its files are read-only, so the Trash can't empty them; run `go clean -modcache`", safety: Safety::Manage },
    Category { base: Base::Home, path: ".rustup/toolchains", title: "Rust toolchains", blurb: "`rustup toolchain list` shows them; `rustup toolchain uninstall` removes old ones", safety: Safety::Manage },
    Category { base: Base::Home, path: ".cache/huggingface", title: "Hugging Face models", blurb: "Downloaded AI models; some are slow to fetch again, and something may still use them", safety: Safety::Review },
    Category { base: Base::Home, path: ".ollama/models", title: "Ollama models", blurb: "Local AI models; `ollama list` shows them, `ollama rm` removes one", safety: Safety::Manage },
    Category { base: Base::Home, path: "Library/Android/sdk/system-images", title: "Android system images", blurb: "Emulator images; remove them in Android Studio's SDK Manager", safety: Safety::Manage },
    Category { base: Base::Home, path: "Movies", title: "Movies", blurb: "Videos and editing projects", safety: Safety::Review },
    Category {
        base: Base::UserTemp,
        path: "X/com.google.Chrome.code_sign_clone",
        title: "Chrome update leftovers",
        blurb: "Old copies Chrome keeps while updating; quit Chrome first",
        safety: Safety::Safe,
    },
    Category { base: Base::Home, path: "Library/Mail", title: "Mail", blurb: "Messages and downloaded attachments", safety: Safety::Review },
];

/// Findings smaller than this aren't worth anyone's attention.
pub const MIN_SIZE: u64 = 50_000_000;

#[derive(Clone, Debug)]
pub struct Finding {
    /// Stable across re-sorts, so a background `frees_of` can find this finding again
    /// after the list has been reordered or pruned.
    pub id: u64,
    pub title: &'static str,
    pub blurb: String,
    pub safety: Safety,
    pub size: u64,
    /// The folder, for a single-location finding.
    pub path: Option<PathBuf>,
    /// Every folder involved (one for a location, many for e.g. node_modules).
    pub nodes: Vec<usize>,
    /// `size` is still the allocated size; what deleting frees is being worked out in
    /// the background (`scan::frees_of`).
    pub pending: bool,
}

pub fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// After nodes were detached by `Tree::remove`: drop findings with no nodes left, prune
/// the detached nodes from the rest, and recompute each size from the live tree. What a
/// deletion really frees stays the cleanup plan's job, at review time.
pub fn prune_after_removal(findings: &mut Vec<Finding>, tree: &Tree) {
    findings.retain_mut(|f| {
        f.nodes.retain(|&ix| tree.nodes[ix].parent.is_some());
        f.size = f.nodes.iter().map(|&ix| tree.nodes[ix].size).sum();
        !f.nodes.is_empty()
    });
    findings.sort_by(|a, b| b.size.cmp(&a.size));
}

/// After one catalog location was re-scanned and grafted into the tree (the Trash,
/// when Finder empties it): recompute that finding from the live tree, adding it when
/// the folder grew past the threshold and dropping it when it shrank below.
pub fn refresh_location(findings: &mut Vec<Finding>, tree: &Tree, bases: &Bases, title: &str) {
    let Some(category) = CATALOG.iter().find(|c| c.title == title) else { return };
    let Some(path) = bases.locate(category) else { return };
    let Some(ix) = tree.find(&path) else { return };
    let size = tree.nodes[ix].size;
    findings.retain(|f| f.title != title);
    if size >= MIN_SIZE {
        findings.push(Finding {
            id: next_id(),
            title: category.title,
            blurb: category.blurb.to_string(),
            safety: category.safety,
            size,
            path: Some(path),
            nodes: vec![ix],
            pending: false,
        });
    }
    findings.sort_by(|a, b| b.size.cmp(&a.size));
}

/// Take everything protected (see `crate::protect`) out of the findings, so
/// none is ever offered. A folder that is protected, or inside something
/// protected, goes; a folder that only holds something protected is offered
/// as what's beside it instead (protect `~/Library/Caches/Homebrew` and the
/// other caches are still offered). Returns what was left out, so the UI can
/// say so.
pub fn shield(findings: &mut Vec<Finding>, tree: &Tree, protected: &[PathBuf]) -> Vec<PathBuf> {
    let mut skipped = Vec::new();
    if protected.is_empty() {
        return skipped;
    }
    findings.retain_mut(|f| {
        if f.nodes.is_empty() {
            // Mid-scan: only the location is known.
            return match &f.path {
                Some(path) if crate::protect::blocking(path, protected).is_some() => {
                    skipped.push(path.clone());
                    false
                }
                _ => true,
            };
        }
        let mut kept = Vec::new();
        for &ix in &f.nodes {
            split(tree, ix, protected, &mut kept, &mut skipped);
        }
        if kept != f.nodes {
            f.nodes = kept;
            f.size = f.nodes.iter().map(|&ix| tree.nodes[ix].size).sum();
        }
        !f.nodes.is_empty()
    });
    findings.sort_by(|a, b| b.size.cmp(&a.size));
    skipped.sort();
    skipped.dedup();
    skipped
}

/// `ix` if nothing protected touches it; nothing if it's protected or inside
/// something protected; else (it holds something protected) its children,
/// each looked at the same way.
fn split(tree: &Tree, ix: usize, protected: &[PathBuf], kept: &mut Vec<usize>, skipped: &mut Vec<PathBuf>) {
    let path = tree.path_of(ix);
    let Some((_, holds)) = crate::protect::how(&path, protected) else {
        kept.push(ix);
        return;
    };
    if !holds || tree.nodes[ix].kind != Kind::Dir {
        skipped.push(path);
        return;
    }
    for &child in &tree.nodes[ix].children {
        if tree.nodes[child].kind != Kind::Other {
            split(tree, child, protected, kept, skipped);
        }
    }
}

/// A hotspot's result as reported mid-scan.
#[derive(Clone, Debug)]
pub struct Early {
    pub category: usize,
    pub path: PathBuf,
    pub size: u64,
    /// Milliseconds after the scan started.
    pub at_ms: u64,
}

pub fn early_findings(early: &[Early]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = early
        .iter()
        .filter(|e| e.size >= MIN_SIZE)
        .map(|e| {
            let category = &CATALOG[e.category];
            Finding {
                id: next_id(),
                title: category.title,
                blurb: category.blurb.to_string(),
                safety: category.safety,
                size: e.size,
                path: Some(e.path.clone()),
                nodes: Vec::new(),
                pending: false,
            }
        })
        .collect();
    findings.sort_by(|a, b| b.size.cmp(&a.size));
    findings
}

/// Findings from a finished scan: the catalog locations, plus every `node_modules`.
/// Sizes start as allocated size, all `pending`: what deleting really frees depends on
/// APFS clone sharing, which the caller works out in the background (`scan::frees_of`).
pub fn from_tree(tree: &Tree, bases: &Bases) -> Vec<Finding> {
    from_tree_min(tree, bases, MIN_SIZE)
}

fn from_tree_min(tree: &Tree, bases: &Bases, min_size: u64) -> Vec<Finding> {
    let mut findings = Vec::new();
    {
        for category in CATALOG {
            let Some(path) = bases.locate(category) else { continue };
            let Some(ix) = tree.find(&path) else { continue };
            let size = tree.nodes[ix].size;
            if size >= min_size {
                findings.push(Finding {
                    id: next_id(),
                    title: category.title,
                    blurb: category.blurb.to_string(),
                    safety: category.safety,
                    size,
                    path: Some(path),
                    nodes: vec![ix],
                    pending: true,
                });
            }
        }
    }

    // Project folders anywhere: node_modules, build outputs, Python
    // environments. Outermost only, so nested ones are counted once. Not
    // inside an app or a tool's own folder (Electron apps, nvm, npm's cache,
    // editor extensions): those are the app's, and "safe to delete" would be
    // a lie there. Nor in the Trash, which has its own finding.
    let mut modules = Vec::new();
    let mut outputs = Vec::new();
    let mut envs = Vec::new();
    let mut stack = vec![Tree::ROOT];
    while let Some(ix) = stack.pop() {
        for &child in &tree.nodes[ix].children {
            let node = &tree.nodes[child];
            if node.kind != Kind::Dir || Tree::is_bundled_name(&node.name) || node.name.as_ref() == ".Trash" {
                continue;
            }
            if node.name.as_ref() == "node_modules" {
                modules.push(child);
            } else if build_output(tree, child) {
                outputs.push(child);
            } else if python_env(tree, child) {
                envs.push(child);
            } else {
                stack.push(child);
            }
        }
    }
    // Clone sharing isn't known here (pnpm installs them as clones), so start from the
    // allocated size and let the caller work out what deleting really frees.
    let mut group = |nodes: Vec<usize>, title: &'static str, blurb: String, safety: Safety| {
        let size: u64 = nodes.iter().map(|&ix| tree.nodes[ix].size).sum();
        if size >= min_size && !nodes.is_empty() {
            findings.push(Finding { id: next_id(), title, blurb, safety, size, path: None, nodes, pending: true });
        }
    };
    let n = modules.len();
    group(modules, "node_modules", format!("In {n} projects; reinstall with your package manager"), Safety::Safe);
    // A folder git tracks files in is source, whatever its name: some teams
    // commit Pods or build. Read-only (`git ls-files`), and only for these.
    let git = Git::new();
    outputs.retain(|&ix| !git.tracks(tree, ix));
    envs.retain(|&ix| !git.tracks(tree, ix));
    let n = outputs.len();
    group(outputs, BUILD_OUTPUTS, format!("In {n} {}; the project's build makes them again", if n == 1 { "project" } else { "projects" }), Safety::Safe);
    let n = envs.len();
    group(envs, "Python environments", format!("{n} virtual {}; recreate with `uv sync` or `pip install -r`", if n == 1 { "environment" } else { "environments" }), Safety::Review);

    findings.sort_by(|a, b| b.size.cmp(&a.size));
    findings
}

pub const BUILD_OUTPUTS: &str = "Build outputs";

/// Build folders, and the files that must sit beside one for Broom to call
/// it the build's: a `target` next to `Cargo.toml` is Cargo's, one in
/// Documents is somebody's. A name alone never decides.
const OUTPUTS: &[(&str, &[&str])] = &[
    ("target", &["Cargo.toml"]),
    (".next", &["package.json"]),
    (".nuxt", &["package.json"]),
    (".svelte-kit", &["package.json"]),
    (".turbo", &["package.json", "turbo.json"]),
    (".parcel-cache", &["package.json"]),
    (".angular", &["angular.json"]),
    ("Pods", &["Podfile"]),
    ("build", &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "pubspec.yaml"]),
    (".gradle", &["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts"]),
    (".build", &["Package.swift"]),
    (".dart_tool", &["pubspec.yaml"]),
    (".zig-cache", &["build.zig"]),
    ("zig-cache", &["build.zig"]),
    ("_build", &["mix.exs"]),
    (".stack-work", &["stack.yaml"]),
];

/// `ix` is a project's build output: a known name with its tool's file
/// beside it, or any folder the tool that made it marked as a cache with a
/// `CACHEDIR.TAG` (cargo does; see bford.info/cachedir).
fn build_output(tree: &Tree, ix: usize) -> bool {
    let name = tree.nodes[ix].name.as_ref();
    let has_file = |dir: usize, file: &str| tree.nodes[dir].children.iter().any(|&c| tree.nodes[c].kind == Kind::File && tree.nodes[c].name.as_ref() == file);
    if has_file(ix, "CACHEDIR.TAG") {
        return true;
    }
    let Some(parent) = tree.nodes[ix].parent else { return false };
    OUTPUTS.iter().any(|(folder, beside)| *folder == name && beside.iter().any(|f| has_file(parent, f)))
}

/// Asks git, read-only, whether it tracks anything in a folder.
struct Git {
    /// Running `git` on a Mac without developer tools pops up an installer;
    /// only ask when there's a real git to ask.
    available: bool,
}

impl Git {
    fn new() -> Self {
        let available = ["/Library/Developer/CommandLineTools/usr/bin/git", "/Applications/Xcode.app/Contents/Developer/usr/bin/git", "/opt/homebrew/bin/git", "/usr/local/bin/git"]
            .iter()
            .any(|p| Path::new(p).exists());
        Git { available }
    }

    /// `ix` sits in a git checkout (a `.git` folder or worktree file above
    /// it) and git tracks at least one file in it. Without git to ask, a
    /// folder in a checkout counts as tracked: unknown is never junk.
    fn tracks(&self, tree: &Tree, ix: usize) -> bool {
        let mut at = tree.nodes[ix].parent;
        let mut in_repo = false;
        while let Some(n) = at {
            if tree.nodes[n].children.iter().any(|&c| tree.nodes[c].name.as_ref() == ".git") {
                in_repo = true;
                break;
            }
            at = tree.nodes[n].parent;
        }
        if !in_repo {
            return false;
        }
        if !self.available {
            return true;
        }
        let path = tree.path_of(ix);
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return true };
        match std::process::Command::new("git").arg("-C").arg(parent).args(["ls-files", "--cached", "-z", "--"]).arg(name).output() {
            Ok(out) if out.status.success() => !out.stdout.is_empty(),
            // Not a checkout after all (a stray .git), or git failed: say tracked.
            _ => true,
        }
    }
}

/// `ix` is a Python virtual environment: `pyvenv.cfg` at its top, which
/// `venv`, `virtualenv` and `uv` all write.
fn python_env(tree: &Tree, ix: usize) -> bool {
    tree.nodes[ix].children.iter().any(|&c| tree.nodes[c].kind == Kind::File && tree.nodes[c].name.as_ref() == "pyvenv.cfg")
}

/// Where the catalog's base folders are, as paths inside a scanned tree.
#[derive(Clone, Debug, Default)]
pub struct Bases {
    pub home: Option<PathBuf>,
    pub user_temp: Option<PathBuf>,
}

impl Bases {
    /// The real locations, as they appear under `tree_root`: as-is, or under the Data
    /// volume when scanning the startup disk.
    pub fn for_root(tree_root: &Path) -> Self {
        let within = |path: PathBuf| -> Option<PathBuf> {
            if path.starts_with(tree_root) {
                return Some(path);
            }
            let mapped = tree_root.join(path.strip_prefix("/").ok()?);
            mapped.exists().then_some(mapped)
        };
        Bases {
            home: std::env::var_os("HOME").map(PathBuf::from).and_then(within),
            user_temp: user_temp_dir().and_then(within),
        }
    }

    pub fn locate(&self, category: &Category) -> Option<PathBuf> {
        let base = match category.base {
            Base::Home => self.home.as_ref(),
            Base::UserTemp => self.user_temp.as_ref(),
        }?;
        Some(base.join(category.path))
    }
}

/// e.g. /private/var/folders/xy/abc123…0000gn: the parent of DARWIN_USER_DIR.
fn user_temp_dir() -> Option<PathBuf> {
    let mut buf = vec![0 as libc::c_char; 1024];
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_DIR, buf.as_mut_ptr(), buf.len()) };
    if len == 0 || len > buf.len() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    std::fs::canonicalize(dir).ok()?.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Node, Progress, scan};

    /// The Trash finding follows the folder: gone when Finder empties it, back when
    /// something big lands in it.
    #[test]
    fn trash_finding_follows_the_folder() {
        let dir = std::env::temp_dir().join(format!("broom-refresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("Users/someone");
        write(&home.join(".Trash/old.dmg"), 300_000);
        write(&home.join("Documents/notes.txt"), 5000);
        let bases = Bases { home: Some(home.clone()), user_temp: None };
        let mut tree = scan(&dir, &Progress::default(), &[], 2);
        let mut findings = from_tree_min(&tree, &bases, 1);
        assert!(findings.iter().any(|f| f.title == "Trash"));

        std::fs::remove_dir_all(home.join(".Trash")).unwrap();
        std::fs::create_dir(home.join(".Trash")).unwrap();
        let trash = tree.find(&home.join(".Trash")).unwrap();
        tree.graft(trash, &scan(&home.join(".Trash"), &Progress::default(), &[], 2));
        refresh_location(&mut findings, &tree, &bases, "Trash");
        assert!(findings.iter().all(|f| f.title != "Trash"), "empty Trash is no finding");

        write(&home.join(".Trash/big.iso"), MIN_SIZE as usize + 1);
        tree.graft(trash, &scan(&home.join(".Trash"), &Progress::default(), &[], 2));
        refresh_location(&mut findings, &tree, &bases, "Trash");
        let f = findings.iter().find(|f| f.title == "Trash").expect("back");
        assert_eq!(f.size, tree.nodes[trash].size);
        assert_eq!(f.nodes, vec![trash]);
        assert!(!f.pending);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn write(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![1u8; bytes]).unwrap();
    }

    #[test]
    fn finds_catalog_locations_and_node_modules_once() {
        let dir = std::env::temp_dir().join(format!("broom-findings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        write(&home.join(".Trash/old.dmg"), 300_000);
        write(&home.join("Downloads/setup.pkg"), 200_000);
        write(&home.join("code/app/node_modules/a/index.js"), 100_000);
        // Nested node_modules must not be counted twice.
        write(&home.join("code/app/node_modules/b/node_modules/c/index.js"), 50_000);
        write(&home.join("code/lib/node_modules/d/index.js"), 70_000);

        let tree = scan(&dir, &Progress::default(), &[], 2);
        let findings = from_tree_min(&tree, &Bases { home: Some(home.clone()), user_temp: None }, 0);
        let by_title = |title: &str| findings.iter().find(|f| f.title == title);

        let trash = by_title("Trash").expect("trash");
        assert_eq!(trash.size, tree.nodes[tree.find(&home.join(".Trash")).unwrap()].size);
        assert!(by_title("Downloads").is_some());

        let modules = by_title("node_modules").expect("node_modules");
        assert_eq!(modules.nodes.len(), 2, "nested node_modules counted once");
        let expected: u64 = ["code/app/node_modules", "code/lib/node_modules"]
            .iter()
            .map(|p| tree.nodes[tree.find(&home.join(p)).unwrap()].size)
            .sum();
        assert_eq!(modules.size, expected);
        // Largest first.
        assert!(findings.windows(2).all(|w| w[0].size >= w[1].size));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Trashing a folder must take its findings with it: detached nodes disappear,
    /// emptied findings are dropped, and the rest are re-sized from the live tree.
    #[test]
    fn prunes_findings_after_removal() {
        let node = |name: &str, size: u64, parent: Option<usize>| Node {
            name: name.into(),
            size,
            kind: Kind::Dir,
            parent,
            children: Vec::new(),
            items: 0,
        };
        let mut nodes = vec![node("root", 1500, None)];
        for (name, size) in [("a", 1000), ("b", 500)] {
            let ix = nodes.len();
            nodes.push(node(name, size, Some(0)));
            nodes[0].children.push(ix);
        }
        let mut tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let finding = |title: &'static str, target: usize, size: u64| Finding {
            id: next_id(),
            title,
            blurb: String::new(),
            safety: Safety::Safe,
            size,
            path: None,
            nodes: vec![target],
            pending: false,
        };
        let mut findings = vec![finding("A", 1, 1000), finding("B", 2, 500)];

        tree.remove(1);
        prune_after_removal(&mut findings, &tree);
        assert!(findings.iter().all(|f| f.title != "A"), "the trashed folder's finding is gone");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].nodes, vec![2], "surviving finding keeps its node");
    }

    /// Protected things are never offered: a protected location's finding
    /// goes, a protected project leaves node_modules, and protecting a folder
    /// above a finding counts as protecting it.
    #[test]
    fn protected_paths_are_never_offered() {
        let dir = std::env::temp_dir().join(format!("broom-shield-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        write(&home.join("Downloads/setup.pkg"), 200_000);
        write(&home.join(".Trash/old.dmg"), 300_000);
        write(&home.join("code/app/node_modules/a/index.js"), 100_000);
        write(&home.join("code/keep/node_modules/b/index.js"), 70_000);
        let tree = scan(&dir, &Progress::default(), &[], 2);
        let bases = Bases { home: Some(home.clone()), user_temp: None };
        let mut findings = from_tree_min(&tree, &bases, 0);
        let skipped = shield(&mut findings, &tree, &[home.join("Downloads"), home.join("code/keep")]);
        assert!(findings.iter().all(|f| f.title != "Downloads"));
        assert!(findings.iter().any(|f| f.title == "Trash"));
        let modules = findings.iter().find(|f| f.title == "node_modules").unwrap();
        assert_eq!(modules.nodes, vec![tree.find(&home.join("code/app/node_modules")).unwrap()]);
        assert_eq!(modules.size, tree.nodes[modules.nodes[0]].size, "re-sized without the protected one");
        assert_eq!(skipped, vec![home.join("Downloads"), home.join("code/keep/node_modules")]);

        // Protecting one thing inside a location offers the rest of it.
        write(&home.join(".Trash/other.zip"), 100_000);
        let tree = scan(&dir, &Progress::default(), &[], 2);
        let mut findings = from_tree_min(&tree, &bases, 0);
        let skipped = shield(&mut findings, &tree, &[home.join(".Trash/old.dmg")]);
        let trash = findings.iter().find(|f| f.title == "Trash").expect("the rest of the Trash");
        assert_eq!(trash.nodes, vec![tree.find(&home.join(".Trash/other.zip")).unwrap()]);
        assert_eq!(trash.size, tree.nodes[trash.nodes[0]].size);
        assert_eq!(skipped, vec![home.join(".Trash/old.dmg")]);

        // Protecting the whole home takes every finding inside it off.
        let mut all = from_tree_min(&tree, &bases, 0);
        shield(&mut all, &tree, &[home.clone()]);
        assert!(all.is_empty(), "{:?}", all.iter().map(|f| f.title).collect::<Vec<_>>());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Build outputs need their tool's file beside them, or a CACHEDIR.TAG;
    /// a folder's name alone never makes it junk.
    #[test]
    fn build_outputs_need_evidence() {
        let dir = std::env::temp_dir().join(format!("broom-outputs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        write(&home.join("code/crate/Cargo.toml"), 10);
        write(&home.join("code/crate/target/debug/app"), 200_000);
        write(&home.join("Documents/target/plan.pdf"), 300_000); // somebody's "target"
        write(&home.join("code/web/package.json"), 10);
        write(&home.join("code/web/.next/cache/x"), 100_000);
        write(&home.join("code/odd/out/CACHEDIR.TAG"), 43);
        write(&home.join("code/odd/out/blob"), 100_000);
        write(&home.join("code/py/.venv/pyvenv.cfg"), 10);
        write(&home.join("code/py/.venv/lib/x.py"), 100_000);
        write(&home.join("code/notes/venv/readme.md"), 100_000); // no pyvenv.cfg
        let tree = scan(&dir, &Progress::default(), &[], 2);
        let findings = from_tree_min(&tree, &Bases { home: Some(home.clone()), user_temp: None }, 0);
        let paths = |title: &str| -> Vec<PathBuf> {
            let mut p: Vec<PathBuf> = findings.iter().filter(|f| f.title == title).flat_map(|f| f.nodes.iter().map(|&ix| tree.path_of(ix))).collect();
            p.sort();
            p
        };
        assert_eq!(paths(BUILD_OUTPUTS), vec![home.join("code/crate/target"), home.join("code/odd/out"), home.join("code/web/.next")]);
        assert_eq!(paths("Python environments"), vec![home.join("code/py/.venv")]);
        assert_eq!(findings.iter().find(|f| f.title == "Python environments").unwrap().safety, Safety::Review);

        // In a git checkout, an output git tracks files in is source.
        let repo = home.join("code/app");
        write(&repo.join("Podfile"), 10);
        write(&repo.join("Pods/Lib/lib.m"), 100_000);
        write(&repo.join("Pods2/Podfile"), 10);
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success());
        git(&["init", "-q"]);
        git(&["add", "Pods"]);
        write(&home.join("code/app2/.git/HEAD"), 10);
        write(&home.join("code/app2/Cargo.toml"), 10);
        write(&home.join("code/app2/target/debug/x"), 100_000);
        let tree = scan(&dir, &Progress::default(), &[], 2);
        let findings = from_tree_min(&tree, &Bases { home: Some(home.clone()), user_temp: None }, 0);
        let outputs: Vec<PathBuf> = findings.iter().filter(|f| f.title == BUILD_OUTPUTS).flat_map(|f| f.nodes.iter().map(|&ix| tree.path_of(ix))).collect();
        assert!(!outputs.contains(&repo.join("Pods")), "tracked Pods are source: {outputs:?}");
        assert!(outputs.contains(&home.join("code/crate/target")), "outside any checkout it's fine");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
