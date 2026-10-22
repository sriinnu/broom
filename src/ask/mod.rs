//! Ask: turn a plain-language request ("delete node_modules", "find big videos") into a
//! selection in the tree.
//!
//! Code does the finding: exact glob, extension and name matching against the scanned
//! tree, with every safety rule in Rust. The model (Jev, with Laya as fallback) only
//! judges *what the words refer to* — intent, which candidate group, scope — and nothing
//! is ever deleted from its answer alone: a delete resolves to Collector items the user
//! confirms as an explicit list of paths.

mod jev;
mod laya;
mod net;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::SharedString;

use crate::classify;
use crate::findings::{Finding, Safety};
use crate::scan::{Kind, Tree, format_size};

pub use net::Http;
pub use net::UreqHttp;

/// Candidate groups offered to the model (and shown as suggestion cards).
pub const MAX_GROUPS: usize = 12;
/// Nodes held per group, largest first.
pub const MAX_NODES_PER_GROUP: usize = 200;
/// Smaller matches aren't worth offering.
pub const MIN_NODE_SIZE: u64 = 1_000_000;
/// A kind-of-thing group (photos, developer files) only counts real space hogs.
pub const MIN_CATEGORY_SIZE: u64 = 100_000_000;
/// Biggest nodes classified by path when a query names a kind of thing.
pub const MAX_CLASSIFY_SWEEP: usize = 20_000;
/// Most nodes one answer may collect.
pub const MAX_COLLECT: usize = 500;
/// A delete answer pre-selects (never trashes) only at or above these.
pub const ACT_CONFIDENCE: f64 = 0.65;
pub const ACT_PROBABILITY: f64 = 0.55;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Delete,
    Locate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    FocusedFolder,
    WholeTree,
}

/// What the provider understood, with the probabilities it based that on.
#[derive(Clone, Debug)]
pub struct Understanding {
    pub intent: Intent,
    /// Index into the candidate groups; `usize::MAX` means "nothing matched".
    pub group: usize,
    /// Every group the words refer to, when the request names more than one
    /// ("node_modules and dist"). Includes `group` when that is set.
    pub groups: Vec<usize>,
    pub scope: Scope,
    pub confidence: f64,
    pub top_probability: f64,
    pub provider: &'static str,
}

/// A set of nodes the query might mean. Built by code; the model only picks one.
#[derive(Clone, Debug)]
pub struct CandidateGroup {
    /// How the group was found; decides what an answer may tick on its own.
    pub source: Source,
    pub label: SharedString,
    /// One line for the provider's state, e.g. "JavaScript dependencies, safe to delete".
    pub detail: String,
    /// Node indices into the results tree, largest first, capped.
    pub nodes: Vec<usize>,
    /// Sum of the (capped) nodes' sizes.
    pub total: u64,
    pub safety: Safety,
}

/// How a candidate group came to be. Findings, known folder names, extensions
/// and worktrees are precise enough for a delete answer to pre-select; a
/// substring or kind match is shown, never ticked by a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Finding,
    Folder,
    Extension,
    Worktree,
    Pattern,
    Kind,
}

impl Source {
    /// May a provider's delete answer tick this group without the user?
    pub fn auto_tick(self) -> bool {
        matches!(self, Source::Finding | Source::Folder | Source::Extension | Source::Worktree)
    }
}

/// Where the query is asked from.
pub struct AskContext {
    pub scan_root: PathBuf,
    /// The folder in view, if not the tree root.
    pub focused: Option<PathBuf>,
}

impl AskContext {
    /// What a provider is told about the root: its name, never the path.
    pub fn root_name(&self) -> String {
        crate::scan::display_name(&self.scan_root)
    }

    /// The folder in view relative to the root, or nothing.
    pub fn focused_name(&self) -> Option<String> {
        let f = self.focused.as_ref()?;
        Some(f.strip_prefix(&self.scan_root).unwrap_or(f).to_string_lossy().into_owned())
    }
}

#[derive(Clone, Debug)]
pub enum ProviderError {
    /// Quiet: no key configured, so "Jev unavailable".
    NoKey,
    /// 401. Never includes the key in its message.
    Unauthorized,
    Http(String),
    BadResponse(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::NoKey => write!(f, "no API key configured"),
            ProviderError::Unauthorized => write!(f, "the API key was rejected"),
            // Http and BadResponse strings are built from status codes and short
            // excerpts only; the key never appears in either.
            ProviderError::Http(message) | ProviderError::BadResponse(message) => write!(f, "{message}"),
        }
    }
}

pub trait Provider: Send + Sync {
    fn configured(&self) -> bool;
    /// Blocking; must run on a background thread. Judges meaning only: it is handed the
    /// candidate groups and never sees absolute paths or any safety policy.
    fn understand(&self, query: &str, ctx: &AskContext, groups: &[CandidateGroup]) -> Result<Understanding, ProviderError>;
}

/// Jev first, Laya on error or low confidence. `Err` means act on exact matches only;
/// `BadResponse("no confident answer")` means the providers replied but nothing cleared
/// the bar, which the caller shows as suggestions rather than an outage.
pub fn resolve(
    providers: &[Box<dyn Provider>],
    query: &str,
    ctx: &AskContext,
    groups: &[CandidateGroup],
) -> Result<Understanding, ProviderError> {
    let mut last = ProviderError::NoKey;
    let mut answered = false;
    for provider in providers {
        if !provider.configured() {
            continue;
        }
        match provider.understand(query, ctx, groups) {
            Ok(u) if acts(&u) => return Ok(u),
            Ok(_) => answered = true,
            Err(error) => last = error,
        }
    }
    Err(if answered { ProviderError::BadResponse("no confident answer".into()) } else { last })
}

fn acts(u: &Understanding) -> bool {
    u.group != usize::MAX && u.confidence >= ACT_CONFIDENCE && u.top_probability >= ACT_PROBABILITY
}

/// The providers to ask, in order.
pub fn providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(jev::Jev { http: Arc::new(UreqHttp) }),
        Box::new(laya::Laya::new(Arc::new(UreqHttp))),
    ]
}

/// Which nodes from a group may be pre-selected for deletion: never the root, detached
/// nodes, volume slices, or a whole home folder, and never more than `MAX_COLLECT`.
pub fn collectable(tree: &Tree, homes: &[usize], group: &CandidateGroup, scope: Scope, focus: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for &ix in &group.nodes {
        if out.len() >= MAX_COLLECT {
            break;
        }
        let node = &tree.nodes[ix];
        if ix == Tree::ROOT || node.parent.is_none() || node.kind == Kind::Other || homes.contains(&ix) {
            continue;
        }
        // A word like "downloads" or "xcode" matches the folder itself. A name match
        // never offers a home's own folders (Downloads, Library…) or Library's
        // top level (Developer, Application Support…): too big to take on a word.
        if group.source == Source::Pattern || group.source == Source::Kind {
            let parent = node.parent.unwrap_or(Tree::ROOT);
            let in_home = homes.contains(&parent);
            let in_library = tree.nodes[parent].name.as_ref() == "Library"
                && tree.nodes[parent].parent.is_some_and(|gp| homes.contains(&gp));
            if in_home || in_library {
                continue;
            }
        }
        if scope == Scope::FocusedFolder && !tree.is_ancestor_or_self(focus, ix) {
            continue;
        }
        out.push(ix);
    }
    out
}

const STOP_WORDS: &[&str] = &[
    "delete", "remove", "clean", "cleanup", "clear", "free", "find", "show", "list", "big", "large", "huge", "old",
    "from", "in", "the", "my", "all", "please", "files", "folders", "folder", "directories", "directory", "dirs", "dir",
    "me", "up", "which", "that", "are", "is", "taking", "take", "space", "disk", "much", "how", "what", "there", "any",
    "give", "get", "rid", "of", "them", "stuff", "junk", "and", "or", "a", "an", "to", "on", "this", "mac", "can",
    "you", "i", "want", "would", "like", "them", "those", "these", "it", "with", "for", "every", "everything",
    // Telugu verbs and demonstratives people type in Latin script.
    "anni", "teesey", "teesei", "teeyi", "teeyandi", "theesey", "teesivey", "cheyyi", "cheyi", "chey", "chupinchu",
    "ante", "avi", "ivi", "idi", "adi", "enni", "ekkada", "pedda", "paatha", "unnayi", "unnai", "ledu",
];

/// Phrases people type that mean one folder name.
const PHRASES: &[(&str, &str)] = &[
    ("node modules", "node_modules"),
    ("nodemodules", "node_modules"),
    ("npm packages", "node_modules"),
    ("derived data", "deriveddata"),
    ("work trees", "worktrees"),
    ("git worktrees", "worktrees"),
    ("git worktree", "worktrees"),
    ("py cache", "pycache"),
    ("virtual envs", "venv"),
    ("virtual env", "venv"),
    ("virtualenvs", "venv"),
    ("virtualenv", "venv"),
    ("cocoapods", "pods"),
    ("build folders", "build"),
    ("build dirs", "build"),
];

/// The words worth matching on; intent words are the model's job.
pub fn tokens(query: &str) -> Vec<String> {
    // Phrases are replaced as whole words only, so "virtual environments" isn't
    // mangled by the "virtual env" entry.
    let mut text = format!(" {} ", query.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "));
    for (phrase, word) in PHRASES {
        text = text.replace(&format!(" {phrase} "), &format!(" {word} "));
    }
    text.split_whitespace()
        .map(|t| t.trim_matches(|c: char| matches!(c, ',' | '.' | '?' | '!' | ';' | '/' | '"' | '\'' | '“' | '”' | '‘' | '’' | '(' | ')')))
        .filter(|t| !t.is_empty() && !STOP_WORDS.contains(t) && t.len() < 64)
        .map(str::to_string)
        .collect()
}

/// Other words a finding answers to, by its catalog title, so "simulators",
/// "email" or "rust" find the right card.
const FINDING_ALIASES: &[(&str, &[&str])] = &[
    ("Trash", &["trash", "bin", "deleted"]),
    ("Downloads", &["download", "installer", "dmg", "pkg"]),
    ("Xcode build files", &["xcode", "deriveddata", "derived", "build", "ios"]),
    ("Xcode archives", &["xcode", "archive", "ios"]),
    ("iOS device support", &["xcode", "ios", "device", "iphone"]),
    ("iOS simulators", &["xcode", "ios", "simulator", "sim", "iphone"]),
    ("iPhone & iPad backups", &["backup", "iphone", "ipad", "ios"]),
    ("Docker", &["docker", "container", "image", "volume"]),
    ("App caches", &["cache", "app"]),
    ("npm cache", &["npm", "cache", "node", "js", "javascript"]),
    ("Cargo registry", &["cargo", "rust", "crate"]),
    ("Gradle caches", &["gradle", "android", "cache", "java", "kotlin"]),
    ("Movies", &["movie", "video", "film", "imovie"]),
    ("Chrome update leftovers", &["chrome", "google", "update", "leftover", "temp", "tmp"]),
    ("Mail", &["mail", "email", "attachment", "message"]),
    ("node_modules", &["node_module", "node", "npm", "pnpm", "yarn", "js", "javascript", "package", "dependency", "dependencie"]),
];

/// Does a finding answer to any of these words? Title words, blurb words and
/// aliases, compared as singular stems.
fn finding_matches(finding: &Finding, words: &[String]) -> bool {
    let title = finding.title.to_lowercase();
    let blurb = finding.blurb.to_lowercase();
    let aliases = FINDING_ALIASES.iter().find(|(t, _)| *t == finding.title).map(|(_, a)| *a).unwrap_or(&[]);
    words.iter().any(|w| {
        let w = w.trim_start_matches("*.").trim_start_matches('.');
        let stem = singular(w);
        title.contains(stem)
            || blurb.split(|c: char| !c.is_alphanumeric() && c != '_').any(|b| !b.is_empty() && singular(b) == stem)
            || aliases.iter().any(|a| *a == stem || *a == w)
    })
}

/// Folders a name match must not reach into: their contents are the tool's, not the user's.
const CONTAINERS: &[&str] = &["node_modules", ".venv", "venv", "Pods", "target", "DerivedData", ".gradle", ".cargo", ".npm"];

/// Inside these a folder may carry a familiar name (an Electron app's own
/// node_modules, a library's cache) without being the user's to delete: a
/// name match there is Review, never Safe.
fn in_bundle(tree: &Tree, ix: usize) -> bool {
    tree.in_bundle(ix)
}

/// How specific a group is, for the dedupe: a finding beats a precise match
/// beats a substring or kind match; ties go to the larger total.
fn precision(source: Source) -> u8 {
    match source {
        Source::Finding => 3,
        Source::Folder | Source::Extension | Source::Worktree => 2,
        Source::Pattern | Source::Kind => 1,
    }
}

/// Folder names developers ask for by name, with what deleting them means. The
/// token on the left is what the (singularised) word must equal; the names on
/// the right are matched exactly, case-insensitively, against folder names, and
/// only the outermost match counts so nested copies aren't double-counted.
/// The sixth field: files whose presence beside the folder earns the Safe
/// verdict (a `target` next to `Cargo.toml` is Cargo's; one in Documents is
/// somebody's). Empty means the name alone decides.
pub const FOLDER_WORDS: &[(&str, &[&str], &str, &str, Safety, &[&str])] = &[
    ("node_module", &["node_modules"], "node_modules", "JavaScript dependencies; `npm install` or `pnpm install` brings them back", Safety::Safe, &["package.json"]),
    ("dist", &["dist"], "dist folders", "Built output; the build script makes it again", Safety::Review, &[]),
    ("build", &["build"], "build folders", "Build output, usually regenerated; check for anything hand-made first", Safety::Review, &[]),
    ("out", &["out"], "out folders", "Exported output (Next.js and friends); the export makes it again", Safety::Review, &[]),
    ("target", &["target"], "target folders", "Cargo build output; `cargo build` recreates it", Safety::Safe, &["Cargo.toml"]),
    ("deriveddata", &["DerivedData"], "Xcode DerivedData", "Xcode rebuilds it on the next build", Safety::Safe, &[]),
    ("pycache", &["__pycache__"], "__pycache__ folders", "Python bytecode caches; recreated on the next run", Safety::Safe, &[]),
    ("venv", &[".venv", "venv"], "Python virtualenvs", "Recreate with `python -m venv` or `uv sync`", Safety::Review, &[]),
    ("pod", &["Pods"], "CocoaPods", "`pod install` recreates them", Safety::Safe, &["Podfile"]),
    ("gradle", &[".gradle"], ".gradle folders", "Gradle caches; re-downloaded on the next build", Safety::Safe, &[]),
    ("turbo", &[".turbo"], ".turbo caches", "Turborepo caches; rebuilt on demand", Safety::Safe, &[]),
    ("coverage", &["coverage"], "coverage folders", "Test coverage reports; regenerated by the test run", Safety::Review, &[]),
    ("vendor", &["vendor"], "vendor folders", "Vendored dependencies; the package manager restores them", Safety::Review, &[]),
    ("cache", &[".cache", "Caches", "cache"], "cache folders", "Caches by name; apps and tools rebuild them", Safety::Review, &[]),
];

/// Folders that are linked git checkouts: a `.git` *file* (not a folder) inside
/// means `git worktree add` or a submodule put it there.
fn is_linked_checkout(tree: &Tree, ix: usize) -> bool {
    tree.nodes[ix].kind == Kind::Dir
        && tree.nodes[ix].children.iter().any(|&c| tree.nodes[c].kind == Kind::File && tree.nodes[c].name.as_ref() == ".git")
}

/// Does any ancestor of `ix` satisfy `pred`?
fn has_ancestor(tree: &Tree, ix: usize, pred: impl Fn(usize) -> bool) -> bool {
    let mut cur = tree.nodes[ix].parent;
    while let Some(p) = cur {
        if pred(p) {
            return true;
        }
        cur = tree.nodes[p].parent;
    }
    false
}

/// Crude singular for table lookups ("videos" → "video"); the original token is untouched.
fn singular(token: &str) -> &str {
    token.strip_suffix('s').unwrap_or(token)
}

const EXTENSION_WORDS: &[(&str, &[&str])] = &[
    ("video", &["mp4", "mov", "mkv", "m4v", "avi"]),
    ("installer", &["dmg", "pkg", "ipa"]),
    ("archive", &["zip", "tar", "gz", "7z", "iso", "rar"]),
    ("image", &["jpg", "jpeg", "png", "heic", "tiff", "raw"]),
    ("audio", &["mp3", "wav", "aac", "flac", "m4a"]),
];

fn category_word(token: &str) -> Option<classify::Category> {
    use classify::Category;
    Some(match singular(token) {
        "app" | "application" => Category::Apps,
        "developer" | "dev" | "code" => Category::Developer,
        "cache" => Category::Caches,
        "media" | "photo" | "movie" | "music" | "video" => Category::Media,
        "download" => Category::Downloads,
        "document" | "doc" => Category::Documents,
        _ => return None,
    })
}

/// Case-insensitive glob with `*` and `?`; a plain word matches anywhere in the name,
/// so "chrome" finds "Google Chrome" and "node" finds "node_modules".
pub fn glob_match(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return name.to_lowercase().contains(&pattern.to_lowercase());
    }
    fn go(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

/// Candidate groups for a query: the findings the words point at, then exact
/// matches by folder name, extension, worktree, substring or kind of thing.
/// Whole-tree, linear passes; runs on a background thread. With no words, the
/// findings alone.
pub fn candidates(tree: &Tree, findings: &[Finding], query: &str) -> Vec<CandidateGroup> {
    let words = tokens(query);
    let mut groups: Vec<CandidateGroup> = Vec::new();
    let timing = std::env::var_os("BROOM_MB_DEBUG").is_some();
    let t0 = std::time::Instant::now();
    let lap = |what: &str| {
        if timing {
            eprintln!("candidates: {what} at {:?}", t0.elapsed());
        }
    };

    // Findings first: they carry exact sizes and a safety verdict. With words in
    // the query only the findings those words name, so a typed match is never
    // crowded out by the catalog. Normalised to the same shape as every other
    // group — largest first, capped — so the dedupe can compare node sets.
    for finding in findings {
        if finding.nodes.is_empty() || (!words.is_empty() && !finding_matches(finding, &words)) {
            continue;
        }
        let mut matched: Vec<(u64, usize)> = finding.nodes.iter().map(|&ix| (tree.nodes[ix].size, ix)).collect();
        matched.sort_by(|a, b| b.0.cmp(&a.0));
        matched.truncate(MAX_NODES_PER_GROUP);
        let total: u64 = matched.iter().map(|&(size, _)| size).sum();
        groups.push(CandidateGroup {
            source: Source::Finding,
            label: format!("{} · {}", finding.title, format_size(total)).into(),
            detail: finding.blurb.clone(),
            nodes: matched.into_iter().map(|(_, ix)| ix).collect(),
            total,
            safety: finding.safety,
        });
    }

    let mut add_matches = |source: Source, label: String, detail: String, safety: Safety, mut matched: Vec<(u64, usize)>| {
        if matched.len() < 2 && matched.iter().all(|&(size, _)| size < MIN_NODE_SIZE) {
            return;
        }
        let everywhere = matched.len() as u64;
        matched.sort_by(|a, b| b.0.cmp(&a.0));
        matched.truncate(MAX_NODES_PER_GROUP);
        let total: u64 = matched.iter().map(|&(size, _)| size).sum();
        let count = matched.len();
        let places = if everywhere > count as u64 {
            format!("{count} of {everywhere} places")
        } else {
            format!("{count} places")
        };
        groups.push(CandidateGroup {
            source,
            label: format!("{label} · {} in {places}", format_size(total)).into(),
            detail,
            total,
            nodes: matched.into_iter().map(|(_, ix)| ix).collect(),
            safety,
        });
    };

    let lower = tree.names();
    let files_with = |ext: &str| -> Vec<(u64, usize)> {
        let suffix = format!(".{}", ext.to_lowercase());
        (1..tree.nodes.len())
            .into_iter()
            .filter(|&ix| {
                let node = &tree.nodes[ix];
                node.kind == Kind::File && node.parent.is_some() && lower.name(ix).ends_with(&suffix)
            })
            .map(|ix| (tree.nodes[ix].size, ix))
            .collect()
    };

    // Extensions. Explicit (`*.dmg`, `.dmg`) and bare ("dmg", when files with that
    // extension exist) get one group each; a word like "videos" gets one merged
    // group for all its extensions, so a model isn't asked to split hairs
    // between .mp4 and .mov.
    let mut handled: Vec<&str> = Vec::new();
    let mut explicit: Vec<String> = Vec::new();
    for word in &words {
        if let Some(ext) = word.strip_prefix("*.").or_else(|| word.strip_prefix('.')).filter(|e| !e.is_empty()) {
            explicit.push(ext.to_string());
            handled.push(word);
        } else if word.len() <= 5
            && word.chars().all(|c| c.is_ascii_alphanumeric())
            && !FOLDER_WORDS.iter().any(|(w, ..)| *w == singular(word))
            && EXTENSION_WORDS.iter().all(|(key, _)| singular(word) != *key)
            && {
                let suffix = format!(".{word}");
                (1..tree.nodes.len()).into_iter().any(|ix| tree.nodes[ix].kind == Kind::File && lower.name(ix).ends_with(&suffix))
            }
        {
            explicit.push(word.clone());
            handled.push(word);
        }
    }
    explicit.sort();
    explicit.dedup();
    lap("findings + bare-extension check");
    for ext in &explicit {
        add_matches(Source::Extension, format!(".{ext} files"), "Files by their extension".into(), Safety::Review, files_with(ext));
    }
    lap("explicit extensions");
    for word in &words {
        if let Some((key, list)) = EXTENSION_WORDS.iter().find(|(key, _)| singular(word) == *key) {
            handled.push(word);
            let mut matched = Vec::new();
            for ext in list.iter() {
                matched.extend(files_with(ext));
            }
            let kinds = list.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(", ");
            add_matches(Source::Extension, format!("{key} files"), format!("Files by their extension ({kinds})"), Safety::Review, matched);
        }
    }

    lap("extension words");
    // Folders developers name directly ("node_modules", "dist", "DerivedData"):
    // exact folder names, outermost only, never inside another tool's folder.
    let mut folder_words: Vec<usize> = Vec::new();
    for word in &words {
        let key = singular(word);
        if let Some(i) = FOLDER_WORDS.iter().position(|(w, ..)| *w == key || *w == word.as_str()) {
            if !folder_words.contains(&i) {
                folder_words.push(i);
            }
            handled.push(word);
        }
    }
    let in_container = |ix: usize| has_ancestor(tree, ix, |p| CONTAINERS.contains(&tree.nodes[p].name.as_ref()));
    for &i in &folder_words {
        let (_, names, label, detail, safety, manifests) = FOLDER_WORDS[i];
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let is_named = |ix: usize| tree.nodes[ix].kind == Kind::Dir && lowered.iter().any(|n| n == lower.name(ix));
        // The Safe verdict needs the tool's manifest beside the folder; a folder
        // with the right name in the wrong place is only Review.
        let earns_safe = |ix: usize| {
            manifests.is_empty()
                || tree.nodes[ix].parent.is_some_and(|p| {
                    tree.nodes[p].children.iter().any(|&c| manifests.iter().any(|m| tree.nodes[c].name.as_ref() == *m))
                })
        };
        let mut safe_nodes = Vec::new();
        let mut review_nodes = Vec::new();
        let hits: Vec<usize> = (1..tree.nodes.len()).into_iter().filter(|&ix| tree.nodes[ix].parent.is_some() && is_named(ix)).collect();
        for ix in hits {
            if has_ancestor(tree, ix, is_named) || in_container(ix) {
                continue;
            }
            if safety == Safety::Safe && earns_safe(ix) && !in_bundle(tree, ix) {
                safe_nodes.push((tree.nodes[ix].size, ix));
            } else {
                review_nodes.push((tree.nodes[ix].size, ix));
            }
        }
        if !safe_nodes.is_empty() {
            add_matches(Source::Folder, label.to_string(), detail.to_string(), Safety::Safe, safe_nodes);
        }
        if !review_nodes.is_empty() {
            let note = if safety == Safety::Safe {
                format!("{detail}. These sit inside an app or a tool's own folder, or have no {} beside them, so look first", if manifests.is_empty() { "manifest".to_string() } else { manifests.join("/") })
            } else {
                detail.to_string()
            };
            add_matches(Source::Folder, label.to_string(), note, Safety::Review, review_nodes);
        }
    }

    lap("folder words");
    // Git worktrees and submodule checkouts: linked `.git` files.
    if words.iter().any(|w| matches!(singular(w), "worktree" | "checkout" | "submodule")) {
        for word in &words {
            if matches!(singular(word), "worktree" | "checkout" | "submodule") {
                handled.push(word);
            }
        }
        let matched: Vec<(u64, usize)> = (1..tree.nodes.len())
            .into_iter()
            .filter(|&ix| tree.nodes[ix].parent.is_some() && is_linked_checkout(tree, ix))
            .map(|ix| (tree.nodes[ix].size, ix))
            .collect();
        add_matches(
            Source::Worktree,
            "Git worktrees".to_string(),
            "Linked checkouts (git worktree or submodules); commits live in the main repo, uncommitted work does not. Run `git worktree prune` afterwards".to_string(),
            Safety::Review,
            matched,
        );
    }

    // Names and globs ("Chrome", "*.dmg" also lands here). Words already handled
    // above don't get a looser substring pass too. Substring groups are shown,
    // never ticked by a model (`Source::Pattern`).
    let mut patterns: Vec<&String> = words
        .iter()
        .filter(|w| w.contains('*') || w.contains('?') || w.len() >= 3)
        .filter(|w| !handled.contains(&w.as_str()))
        .collect();
    patterns.sort();
    patterns.dedup();
    for pattern in patterns {
        let plain = !pattern.contains('*') && !pattern.contains('?');
        let needle = pattern.to_lowercase();
        let matched: Vec<(u64, usize)> = (1..tree.nodes.len())
            .into_iter()
            .filter(|&ix| {
                tree.nodes[ix].parent.is_some()
                    && if plain { lower.name(ix).contains(needle.as_str()) } else { glob_match(&needle, lower.name(ix)) }
            })
            .map(|ix| (tree.nodes[ix].size, ix))
            .collect();
        add_matches(
            Source::Pattern,
            pattern.trim_matches('*').to_string(),
            "Folders and files whose name contains this".into(),
            Safety::Review,
            matched,
        );
    }

    lap("patterns");
    // Kinds of things ("photos", "developer files"): classify the biggest folders.
    let categories: Vec<classify::Category> = words.iter().filter_map(|w| category_word(w)).collect();
    if !categories.is_empty() {
        let mut biggest: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
        for (ix, node) in tree.nodes.iter().enumerate().skip(1) {
            if node.size >= MIN_CATEGORY_SIZE && node.parent.is_some() && node.kind == Kind::Dir {
                biggest.push(Reverse((node.size, ix)));
                if biggest.len() > MAX_CLASSIFY_SWEEP {
                    biggest.pop();
                }
            }
        }
        let swept: Vec<(u64, usize)> = biggest.into_sorted_vec().into_iter().map(|Reverse(v)| v).collect();
        for category in categories {
            let mut nodes = Vec::new();
            for &(size, ix) in &swept {
                if classify::classify(&tree.path_of(ix), false) == category {
                    nodes.push((size, ix));
                    if nodes.len() >= MAX_NODES_PER_GROUP {
                        break;
                    }
                }
            }
            add_matches(
                Source::Kind,
                category.label().to_string(),
                "Folders of this kind, by what they contain".into(),
                Safety::Review,
                nodes,
            );
        }
    }

    lap("kinds");
    // Near-identical selections collapse. A finding always wins over a match
    // that found the same nodes (its verdict and blurb are the point), a precise
    // match (folder name, extension, worktree) over a substring one; among equals
    // the larger wins. Overlap is measured against the smaller set.
    groups.sort_by(|a, b| precision(b.source).cmp(&precision(a.source)).then(b.total.cmp(&a.total)));
    let keys: Vec<Vec<usize>> = groups
        .iter()
        .map(|g| {
            let mut k = g.nodes.clone();
            k.sort();
            k
        })
        .collect();
    let mut keep = vec![true; groups.len()];
    for i in 0..groups.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..groups.len() {
            if !keep[j] {
                continue;
            }
            let shared = keys[j].iter().filter(|ix| keys[i].binary_search(ix).is_ok()).count();
            if shared * 10 >= keys[i].len().min(keys[j].len()) * 8 {
                keep[j] = false;
            }
        }
    }
    let mut kept = 0;
    groups.retain(|_| {
        kept += 1;
        keep[kept - 1]
    });
    // Back to largest first for display, findings and matches alike.
    groups.sort_by(|a, b| b.total.cmp(&a.total));
    groups.truncate(MAX_GROUPS);
    groups
}

/// `None` outer = not looked up yet; inner = the answer. Reset by `invalidate_keys`
/// when Settings writes or removes a key, so the app picks it up without a restart.
static JEV_KEY: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);
static LAYA_KEY: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

/// Env first (works under `cargo run`), then the keychain (app bundles don't inherit
/// the shell). Cached; the key goes only into the Authorization header, never into a
/// log line or an error message.
pub fn jev_key() -> Option<String> {
    let mut slot = JEV_KEY.lock().unwrap();
    slot.get_or_insert_with(|| {
        env_key(&["TYPESAFE_API_KEY", "JEV_API_KEY"]).or_else(|| crate::secrets::read(crate::secrets::JEV_SERVICE))
    })
    .clone()
}

/// Laya asks for no key from the user; this is the optional one — Laya's own
/// env name, or the Vercel gateway's — that gets attached if it happens to be
/// set. `None` sends the request without an Authorization header.
pub fn laya_key() -> Option<String> {
    let mut slot = LAYA_KEY.lock().unwrap();
    slot.get_or_insert_with(|| {
        env_key(&["LAYA_API_KEY", "AI_GATEWAY_API_KEY"]).or_else(|| crate::secrets::read(crate::secrets::LAYA_SERVICE))
    })
    .clone()
}

/// Forget both cached keys — the next lookup goes back to env + keychain.
pub fn invalidate_keys() {
    *JEV_KEY.lock().unwrap() = None;
    *LAYA_KEY.lock().unwrap() = None;
}

fn env_key(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|key| !key.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Node;

    fn node(name: &str, size: u64, kind: Kind, parent: Option<usize>) -> Node {
        Node { name: name.into(), size, kind, parent, children: Vec::new(), items: 1 }
    }

    /// root → code/app/node_modules (3 GB), code/lib/node_modules (1 GB),
    /// Downloads/a.dmg (2 GB), Downloads/b.dmg (1 GB), Photos.photoslibrary (5 GB).
    fn tree() -> Tree {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let push = |nodes: &mut Vec<Node>, name: &str, size: u64, kind: Kind, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, kind, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let code = push(&mut nodes, "code", 0, Kind::Dir, 0);
        let app = push(&mut nodes, "app", 0, Kind::Dir, code);
        let _ = push(&mut nodes, "package.json", 1_000, Kind::File, app);
        let _ = push(&mut nodes, "node_modules", 3_000_000_000, Kind::Dir, app);
        let lib = push(&mut nodes, "lib", 0, Kind::Dir, code);
        let _ = push(&mut nodes, "package.json", 1_000, Kind::File, lib);
        let _ = push(&mut nodes, "node_modules", 1_000_000_000, Kind::Dir, lib);
        let downloads = push(&mut nodes, "Downloads", 3_000_000_000, Kind::Dir, 0);
        let _ = push(&mut nodes, "a.dmg", 2_000_000_000, Kind::File, downloads);
        let _ = push(&mut nodes, "b.dmg", 1_000_000_000, Kind::File, downloads);
        let _ = push(&mut nodes, "Photos.photoslibrary", 5_000_000_000, Kind::Dir, 0);
        Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() }
    }

    fn finding(title: &'static str, nodes: Vec<usize>, size: u64) -> Finding {
        Finding {
            id: 1,
            title,
            blurb: "known space hog".into(),
            safety: Safety::Safe,
            size,
            path: None,
            nodes,
            pending: false,
        }
    }

    #[test]
    fn globs_and_substrings() {
        assert!(glob_match("node_modules", "code/node_modules"));
        assert!(glob_match("node", "node_modules"));
        assert!(glob_match("chrome", "Google Chrome"));
        assert!(glob_match("*.dmg", "A.DMG"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*.dmg", "a.dmg.zip"));
        assert!(!glob_match("zzz", "node_modules"));
    }

    #[test]
    fn finds_name_extension_and_finding_groups() {
        let tree = tree();
        let downloads_ix = tree.nodes.iter().position(|n| n.name.as_ref() == "Downloads").unwrap();
        let findings = vec![finding("Downloads", vec![downloads_ix], 3_000_000_000)];
        let groups = candidates(&tree, &findings, "delete node_modules and *.dmg");

        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_ref()).collect();
        assert!(labels.iter().any(|l| l.contains("node_modules")), "{labels:?}");
        assert!(labels.iter().any(|l| l.contains(".dmg")), "{labels:?}");
        assert!(labels.iter().any(|l| l.contains("Downloads")), "{labels:?}");

        let modules = groups.iter().find(|g| g.label.contains("node_modules")).unwrap();
        assert_eq!(modules.nodes.len(), 2, "both node_modules, no nesting here");
        assert_eq!(modules.total, 4_000_000_000);
        assert_eq!(modules.safety, Safety::Safe, "node_modules is a known folder: safe, package manager restores it");
        let dmgs = groups.iter().find(|g| g.label.contains(".dmg")).unwrap();
        assert_eq!(dmgs.nodes.len(), 2);
        let downloads = groups.iter().find(|g| g.label.contains("Downloads")).unwrap();
        assert!(downloads.safety == Safety::Safe);
        // Findings groups are largest-first like every other group.
        let sizes: Vec<u64> = downloads.nodes.iter().map(|&ix| tree.nodes[ix].size).collect();
        let mut sorted = sizes.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(sizes, sorted, "findings passthrough is largest-first");
    }

    #[test]
    fn findings_groups_are_largest_first() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, Kind::Dir, Some(0)));
            nodes[0].children.push(ix);
            ix
        };
        let small = add(&mut nodes, "small", 100_000_000);
        let mid = add(&mut nodes, "mid", 500_000_000);
        let big = add(&mut nodes, "big", 900_000_000);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        // Filed in traversal (ascending-size) order on purpose.
        let finding = finding("hogs", vec![small, mid, big], 1_500_000_000);
        // No words: the findings alone, as suggestions.
        let groups = candidates(&tree, &[finding], "");
        assert_eq!(groups[0].nodes, vec![big, mid, small], "largest first, not traversal order");
        assert_eq!(groups[0].total, 1_500_000_000);
    }

    #[test]
    fn caps_groups_and_nodes() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        for i in 0..600 {
            let ix = nodes.len();
            let _ = i;
            nodes.push(node("node_modules", 5_000_000, Kind::Dir, Some(0)));
            nodes[0].children.push(ix);
        }
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let groups = candidates(&tree, &[], "node_modules");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].nodes.len(), MAX_NODES_PER_GROUP, "nodes are capped");
    }

    #[test]
    fn plain_english_lands_on_folder_names() {
        let tree = tree();
        let groups = candidates(&tree, &[], "show me all the node modules which are taking up space");
        let labels: Vec<&str> = groups.iter().map(|g| g.label.as_ref()).collect();
        assert!(labels.iter().any(|l| l.starts_with("node_modules")), "{labels:?}");
        // The exact-name pass replaces the looser substring pass for the same word.
        assert_eq!(labels.iter().filter(|l| l.contains("node")).count(), 1, "{labels:?}");
    }

    #[test]
    fn worktrees_are_folders_with_a_git_file() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64, kind: Kind, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, kind, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let main_repo = add(&mut nodes, "broom", 3_000_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 1_000_000, Kind::Dir, main_repo);
        let wt = add(&mut nodes, "broom-feature", 2_000_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 100, Kind::File, wt);
        let wt2 = add(&mut nodes, "broom-fix", 2_500_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 100, Kind::File, wt2);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let groups = candidates(&tree, &[], "all worktrees");
        let group = groups.iter().find(|g| g.label.starts_with("Git worktrees")).expect("a worktrees group");
        assert_eq!(group.nodes, vec![wt2, wt], "linked checkouts only, largest first; the main repo stays");
        assert_eq!(group.safety, Safety::Review);
    }

    #[test]
    fn nested_named_folders_count_once() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, Kind::Dir, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let outer = add(&mut nodes, "dist", 5_000_000, 0);
        let _inner = add(&mut nodes, "dist", 2_000_000, outer);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let groups = candidates(&tree, &[], "dist folders");
        assert_eq!(groups.len(), 1, "{:?}", groups.iter().map(|g| g.label.clone()).collect::<Vec<_>>());
        assert_eq!(groups[0].nodes, vec![outer]);
        assert!(groups[0].label.starts_with("dist folders"), "{}", groups[0].label);
    }

    #[test]
    fn place_words_are_shown_not_ticked() {
        let tree = tree();
        let groups = candidates(&tree, &[], "old dmg files in Downloads");
        let downloads = groups.iter().find(|g| g.label.starts_with("downloads")).expect("a substring group for the folder");
        assert_eq!(downloads.source, Source::Pattern);
        assert!(!downloads.source.auto_tick(), "a name match is never pre-selected by a model");
        let dmg = groups.iter().find(|g| g.label.starts_with(".dmg files")).expect("bare 'dmg' is an extension when such files exist");
        assert_eq!(dmg.source, Source::Extension);
        assert_eq!(dmg.nodes.len(), 2);
    }

    #[test]
    fn name_match_never_offers_a_homes_own_folders() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64, kind: Kind, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, kind, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let home = add(&mut nodes, "sam", 0, Kind::Dir, 0);
        let downloads = add(&mut nodes, "Downloads", 5_000_000_000, Kind::Dir, home);
        let inner = add(&mut nodes, "downloads-old", 1_000_000_000, Kind::Dir, downloads);
        let library = add(&mut nodes, "Library", 0, Kind::Dir, home);
        let developer = add(&mut nodes, "Developer", 2_000_000_000, Kind::Dir, library);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let group = CandidateGroup { source: Source::Pattern, label: "x".into(), detail: String::new(), nodes: vec![downloads, inner, developer], total: 0, safety: Safety::Review };
        assert_eq!(collectable(&tree, &[home], &group, Scope::WholeTree, 0), vec![inner]);
        let finding = CandidateGroup { source: Source::Finding, nodes: vec![downloads], ..group };
        assert_eq!(collectable(&tree, &[home], &finding, Scope::WholeTree, 0), vec![downloads], "a finding may name the folder itself");
    }

    #[test]
    fn worktree_group_survives_a_broader_name_match() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64, kind: Kind, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, kind, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let main_repo = add(&mut nodes, "broom", 3_000_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 1_000_000, Kind::Dir, main_repo);
        let wt = add(&mut nodes, "broom-feature", 2_000_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 100, Kind::File, wt);
        let wt2 = add(&mut nodes, "broom-fix", 2_500_000, Kind::Dir, 0);
        add(&mut nodes, ".git", 100, Kind::File, wt2);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let groups = candidates(&tree, &[], "delete broom worktrees");
        let wts = groups.iter().find(|g| g.source == Source::Worktree).expect("the precise group wins the dedupe");
        assert_eq!(wts.nodes, vec![wt2, wt]);
        assert!(!groups.iter().any(|g| g.source == Source::Pattern), "the broader substring group is the duplicate");
    }

    #[test]
    fn safe_needs_the_manifest_and_stays_out_of_apps() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, size: u64, kind: Kind, parent: usize| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, size, kind, Some(parent)));
            nodes[parent].children.push(ix);
            ix
        };
        let docs = add(&mut nodes, "Documents", 0, Kind::Dir, 0);
        let stray = add(&mut nodes, "target", 5_000_000, Kind::Dir, docs);
        let proj = add(&mut nodes, "proj", 0, Kind::Dir, 0);
        add(&mut nodes, "Cargo.toml", 100, Kind::File, proj);
        let real = add(&mut nodes, "target", 7_000_000, Kind::Dir, proj);
        let app = add(&mut nodes, "Slack.app", 0, Kind::Dir, 0);
        let res = add(&mut nodes, "Resources", 0, Kind::Dir, app);
        add(&mut nodes, "package.json", 100, Kind::File, res);
        let bundled = add(&mut nodes, "node_modules", 9_000_000, Kind::Dir, res);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let groups = candidates(&tree, &[], "target folders and node_modules");
        let safe_target = groups.iter().find(|g| g.label.starts_with("target") && g.safety == Safety::Safe).expect("cargo's target is safe");
        assert_eq!(safe_target.nodes, vec![real]);
        let review_target = groups.iter().find(|g| g.label.starts_with("target") && g.safety == Safety::Review).expect("a stray target is review");
        assert_eq!(review_target.nodes, vec![stray]);
        let nm = groups.iter().find(|g| g.label.starts_with("node_modules")).expect("the app's node_modules is listed");
        assert_eq!((nm.nodes.clone(), nm.safety), (vec![bundled], Safety::Review), "inside an .app it is never safe");
    }

    #[test]
    fn tree_remove_detaches_the_whole_subtree() {
        let mut t = tree();
        let downloads = t.nodes.iter().position(|n| n.name.as_ref() == "Downloads").unwrap();
        let dmg = t.nodes.iter().position(|n| n.name.as_ref() == "a.dmg").unwrap();
        t.remove(downloads);
        assert!(t.nodes[dmg].parent.is_none(), "children of a removed folder are gone too");
        assert!(t.nodes[Tree::ROOT].children.iter().all(|&c| c != downloads));
    }

    #[test]
    fn collectable_refuses_risky_nodes() {
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let add = |nodes: &mut Vec<Node>, name: &str, kind: Kind| -> usize {
            let ix = nodes.len();
            nodes.push(node(name, 1, kind, Some(0)));
            nodes[0].children.push(ix);
            ix
        };
        let home = add(&mut nodes, "sam", Kind::Dir);
        let plain = add(&mut nodes, "node_modules", Kind::Dir);
        let slice = add(&mut nodes, "macOS", Kind::Other);
        nodes.push(node("detached", 1, Kind::Dir, None));
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let group = CandidateGroup {
            source: Source::Folder,
            label: "x".into(),
            detail: String::new(),
            nodes: vec![Tree::ROOT, home, plain, slice, 4],
            total: 5,
            safety: Safety::Review,
        };
        assert_eq!(collectable(&tree, &[home], &group, Scope::WholeTree, Tree::ROOT), vec![plain]);

        // Focused scope keeps only nodes inside the focused folder.
        let mut nodes = vec![node("root", 0, Kind::Dir, None)];
        let inner = add(&mut nodes, "inside", Kind::Dir);
        let outside = add(&mut nodes, "outside", Kind::Dir);
        let tree = Tree { root_path: PathBuf::from("/t"), nodes, errors: 0, cloud_only: 0, index: Default::default() };
        let group = CandidateGroup {
            nodes: vec![inner, outside],
            total: 2,
            ..group
        };
        assert_eq!(collectable(&tree, &[], &group, Scope::FocusedFolder, inner), vec![inner]);
    }

    struct Fake {
        name: &'static str,
        answer: Result<Understanding, ProviderError>,
    }

    impl Provider for Fake {
        fn configured(&self) -> bool {
            true
        }
        fn understand(&self, _: &str, _: &AskContext, groups: &[CandidateGroup]) -> Result<Understanding, ProviderError> {
            // Group answers must point at a real group to be actionable.
            assert!(self.answer.as_ref().map_or(true, |u| u.group == usize::MAX || u.group < groups.len()));
            self.answer.clone().map(|mut u| { u.provider = self.name; u })
        }
    }

    fn understanding(confidence: f64) -> Understanding {
        Understanding {
            intent: Intent::Delete,
            group: 0,
            groups: vec![0],
            scope: Scope::WholeTree,
            confidence,
            top_probability: confidence,
            provider: "fake",
        }
    }

    #[test]
    fn resolve_tries_providers_in_order() {
        let ctx = AskContext { scan_root: PathBuf::from("/t"), focused: None };
        let groups = vec![CandidateGroup {
            source: Source::Folder,
            label: "g".into(),
            detail: String::new(),
            nodes: vec![1],
            total: 1,
            safety: Safety::Review,
        }];
        let good = Ok(understanding(0.9));
        let shaky = Ok(understanding(0.3));
        let down = Err(ProviderError::Http("unreachable".into()));

        assert!(resolve(&[Box::new(Fake { name: "a", answer: good.clone() })], "q", &ctx, &groups).is_ok());
        // Low confidence on the first provider falls through to the second.
        let fell_through = resolve(
            &[Box::new(Fake { name: "a", answer: shaky }), Box::new(Fake { name: "b", answer: good.clone() })],
            "q",
            &ctx,
            &groups,
        );
        assert_eq!(fell_through.unwrap().provider, "b");
        // So does an error.
        let after_error = resolve(
            &[Box::new(Fake { name: "a", answer: down }), Box::new(Fake { name: "b", answer: good })],
            "q",
            &ctx,
            &groups,
        );
        assert_eq!(after_error.unwrap().provider, "b");
        // All failed: the caller falls back to exact matches.
        assert!(resolve(
            &[Box::new(Fake { name: "a", answer: Err(ProviderError::Unauthorized) })],
            "q",
            &ctx,
            &groups
        )
        .is_err());
        // A confident "nothing matched" is not actionable.
        let none = Ok(Understanding { group: usize::MAX, ..understanding(0.95) });
        assert!(resolve(&[Box::new(Fake { name: "a", answer: none })], "q", &ctx, &groups).is_err());
    }

    #[test]
    fn no_key_material_in_error_messages() {
        // Errors are built from status and short excerpts only; nothing echoes a key.
        let error = ProviderError::Http("status 401 from https://api.typesafe.ai".into());
        assert!(!error.to_string().contains("Bearer"));
        assert!(!ProviderError::Unauthorized.to_string().contains("Bearer"));
    }

    /// One real request against whichever provider is configured — the contract check
    /// unit tests can't do. Prints the parsed understanding (or the error) only; the key
    /// goes from env/keychain to the Authorization header and is never surfaced. Run
    /// explicitly: `cargo test -- --ignored live`.
    #[test]
    #[ignore = "spends a real API call"]
    fn live_roundtrip() {
        let providers = providers();
        if !providers.iter().any(|p| p.configured()) {
            eprintln!("no provider key configured; nothing to check");
            return;
        }
        let tree = tree();
        let groups = candidates(&tree, &[], "delete node_modules");
        assert!(!groups.is_empty());
        let ctx = AskContext { scan_root: PathBuf::from("/t"), focused: None };
        match resolve(&providers, "delete node_modules", &ctx, &groups) {
            Ok(u) => {
                assert!(u.group == usize::MAX || u.group < groups.len());
                assert!((0.0..=1.0).contains(&u.confidence), "{}", u.confidence);
                println!("{} answered: intent={:?} group={} scope={:?} confidence={:.2} p={:.2}", u.provider, u.intent, u.group, u.scope, u.confidence, u.top_probability);
            }
            Err(error) => println!("resolve fell back: {error}"),
        }
    }
}
