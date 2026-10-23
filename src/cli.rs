//! The headless side of Broom, for scripts and agents.
//!
//! ```text
//! broom scan  [PATH] [--json]                       what's on the disk: totals, biggest folders, findings
//! broom find  QUERY… [--root PATH] [--ask] [--json] the same groups the menu-bar panel would show
//! broom clean QUERY… [--root PATH] [--all] [--yes]  find, then move the safe groups to the Trash
//! broom trash PATH… --yes [--json]                  move exactly these paths to the Trash
//! ```
//!
//! Same scanner, same ask engine, same safety rules as the window: nothing is
//! deleted outright, only moved to the Trash, and `clean` without `--yes` is a
//! dry run. `--json` prints one object on stdout for a program to read; without
//! it the output is for a person. Exit codes: 0 done, 1 something failed,
//! 2 usage. Progress goes to stderr, never stdout.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use crate::ask::{self, CandidateGroup, Scope, Source};
use crate::findings::{self, Finding, Safety};
use crate::scan::{self, Kind, Tree, format_count, format_size};

const USAGE: &str = "\
broom: ask your Mac what's eating the disk, tick what should go, trash it.

  broom                          open the app
  broom PATH                     open the app scanning PATH
  broom scan  [PATH] [--json]    headless scan: totals, biggest folders, findings
  broom find  QUERY… [--root PATH] [--ask] [--json]
                                 what the panel would show for QUERY (default root: home)
  broom clean QUERY… [--root PATH] [--all] [--yes] [--json]
                                 find, then move the safe groups to the Trash (dry run without --yes)
  broom trash PATH… --yes [--json]
                                 move exactly these paths to the Trash

  --json   one JSON object on stdout; progress stays on stderr
  --ask    also consult the configured reader (Jev/Laya) and report its answer
  --all    clean: include groups marked review, not only safe ones
  --yes    actually move things; without it clean and trash only print the plan

Queries are plain words: \"node modules\", \"dist folders\", \"worktrees\", \"old dmg in downloads\".
";

/// Handle a CLI invocation. Returns `None` when the arguments are for the app
/// (no subcommand), otherwise the process exit code.
pub fn run(args: &[String]) -> Option<i32> {
    let cmd = args.get(1).map(String::as_str)?;
    let rest = &args[2..];
    Some(match cmd {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            0
        }
        "scan" => cmd_scan(rest),
        "find" => cmd_find(rest),
        "clean" => cmd_clean(rest),
        "trash" => cmd_trash(rest),
        _ => return None,
    })
}

struct Opts {
    words: Vec<String>,
    root: Option<PathBuf>,
    json: bool,
    ask: bool,
    all: bool,
    yes: bool,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { words: Vec::new(), root: None, json: false, ask: false, all: false, yes: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => o.json = true,
            "--ask" => o.ask = true,
            "--all" => o.all = true,
            "--yes" | "-y" => o.yes = true,
            "--root" => o.root = Some(PathBuf::from(it.next().ok_or("--root needs a path")?)),
            s if s.starts_with("--root=") => o.root = Some(PathBuf::from(&s[7..])),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => o.words.push(a.clone()),
        }
    }
    Ok(o)
}

fn usage_error(msg: &str) -> i32 {
    eprintln!("broom: {msg}\n");
    eprint!("{USAGE}");
    2
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn tilde(path: &Path) -> String {
    let h = home();
    match path.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

struct Scanned {
    tree: Tree,
    findings: Vec<Finding>,
    files: u64,
    seconds: f64,
}

/// One full scan, the way the window does it: same excludes and outline depth
/// from the settings file, findings resolved from the finished tree.
fn scan_root(root: &Path, quiet: bool) -> Result<Scanned, String> {
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let settings = crate::settings::load().settings;
    if !quiet {
        eprintln!("scanning {}…", tilde(&root));
    }
    let t0 = Instant::now();
    let progress = scan::Progress::default();
    let tree = scan::scan(&root, &progress, &settings.exclude_paths(), settings.outline_depth);
    let findings = findings::from_tree(&tree, &findings::Bases::for_root(&tree.root_path));
    let files = progress.files.load(std::sync::atomic::Ordering::Relaxed);
    let seconds = t0.elapsed().as_secs_f64();
    if !quiet {
        eprintln!("{} files, {} in {seconds:.1}s", format_count(files), format_size(tree.nodes[Tree::ROOT].size));
    }
    Ok(Scanned { tree, findings, files, seconds })
}

fn safety_str(s: Safety) -> &'static str {
    match s {
        Safety::Safe => "safe",
        Safety::Review => "review",
    }
}

fn source_str(s: Source) -> &'static str {
    match s {
        Source::Finding => "finding",
        Source::Folder => "folder",
        Source::Extension => "extension",
        Source::Worktree => "worktree",
        Source::Pattern => "pattern",
        Source::Kind => "kind",
    }
}

fn node_json(tree: &Tree, ix: usize) -> Value {
    let n = &tree.nodes[ix];
    json!({ "path": tree.path_of(ix), "bytes": n.size, "items": n.items })
}

fn finding_json(tree: &Tree, f: &Finding) -> Value {
    json!({
        "title": f.title,
        "safety": safety_str(f.safety),
        "bytes": f.size,
        "why": f.blurb,
        "paths": f.nodes.iter().map(|&ix| tree.path_of(ix)).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------- scan

fn cmd_scan(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    if o.words.len() > 1 {
        return usage_error("scan takes at most one path");
    }
    let root = o.words.first().map(PathBuf::from).or(o.root).unwrap_or_else(home);
    let s = match scan_root(&root, o.json) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("broom: {e}");
            return 1;
        }
    };
    let tree = &s.tree;
    let top: Vec<usize> = tree.nodes[Tree::ROOT].children.iter().copied().take(25).collect();
    if o.json {
        let out = json!({
            "root": tree.root_path,
            "bytes": tree.nodes[Tree::ROOT].size,
            "files": s.files,
            "errors": tree.errors,
            "cloud_only": tree.cloud_only,
            "seconds": s.seconds,
            "top": top.iter().map(|&ix| node_json(tree, ix)).collect::<Vec<_>>(),
            "findings": s.findings.iter().map(|f| finding_json(tree, f)).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return 0;
    }
    println!("{}  {}  {} files", tilde(&tree.root_path), format_size(tree.nodes[Tree::ROOT].size), format_count(s.files));
    if tree.errors > 0 {
        println!("{} items couldn't be read (permissions)", format_count(tree.errors));
    }
    println!();
    for &ix in &top {
        let n = &tree.nodes[ix];
        let slash = if n.kind == Kind::Dir { "/" } else { "" };
        println!("{:>10}  {}{}", format_size(n.size), n.name, slash);
    }
    if !s.findings.is_empty() {
        println!("\nFindings");
        for f in &s.findings {
            println!("{:>10}  {:<7} {}  · {}", format_size(f.size), safety_str(f.safety), f.title, f.blurb);
        }
    }
    0
}

// ---------------------------------------------------------------- find

struct Found {
    scanned: Scanned,
    groups: Vec<CandidateGroup>,
    /// Per group, the folders that may actually be collected (never the root, a
    /// home folder or a volume slice).
    folders: Vec<Vec<usize>>,
    reader: Option<Result<ask::Understanding, ask::ProviderError>>,
}

fn find(query: &str, root: &Path, ask_reader: bool, quiet: bool) -> Result<Found, String> {
    let scanned = scan_root(root, quiet)?;
    let groups = ask::candidates(&scanned.tree, &scanned.findings, query);
    let homes: Vec<usize> = scanned.tree.find(&home()).into_iter().collect();
    let folders = groups.iter().map(|g| ask::collectable(&scanned.tree, &homes, g, Scope::WholeTree, Tree::ROOT)).collect();
    let reader = ask_reader.then(|| {
        let ctx = ask::AskContext { scan_root: scanned.tree.root_path.clone(), focused: None };
        ask::resolve(&ask::providers(), query, &ctx, &groups)
    });
    Ok(Found { scanned, groups, folders, reader })
}

fn group_json(tree: &Tree, ix: usize, g: &CandidateGroup, folders: &[usize]) -> Value {
    json!({
        "index": ix,
        "label": g.label.to_string(),
        "why": g.detail,
        "source": source_str(g.source),
        "safety": safety_str(g.safety),
        "precise": g.source.auto_tick(),
        "bytes": folders.iter().map(|&n| tree.nodes[n].size).sum::<u64>(),
        "folders": folders.iter().map(|&n| node_json(tree, n)).collect::<Vec<_>>(),
    })
}

fn reader_json(r: &Option<Result<ask::Understanding, ask::ProviderError>>) -> Value {
    match r {
        None => Value::Null,
        Some(Ok(u)) => json!({
            "provider": u.provider,
            "intent": match u.intent { ask::Intent::Delete => "delete", ask::Intent::Locate => "locate" },
            "groups": u.groups,
            "confidence": u.confidence,
        }),
        Some(Err(e)) => json!({ "error": e.to_string() }),
    }
}

fn found_json(f: &Found, query: &str) -> Value {
    let tree = &f.scanned.tree;
    json!({
        "root": tree.root_path,
        "query": query,
        "seconds": f.scanned.seconds,
        "groups": f.groups.iter().enumerate().map(|(i, g)| group_json(tree, i, g, &f.folders[i])).collect::<Vec<_>>(),
        "reader": reader_json(&f.reader),
    })
}

fn print_found(f: &Found) {
    let tree = &f.scanned.tree;
    if f.groups.is_empty() {
        println!("nothing matched");
        return;
    }
    for (i, g) in f.groups.iter().enumerate() {
        let folders = &f.folders[i];
        let total: u64 = folders.iter().map(|&n| tree.nodes[n].size).sum();
        println!("[{i}] {}  {}  {}  · {}", g.label, format_size(total), safety_str(g.safety), g.detail);
        for &n in folders.iter().take(12) {
            println!("    {:>10}  {}", format_size(tree.nodes[n].size), tilde(&tree.path_of(n)));
        }
        if folders.len() > 12 {
            println!("    … {} more", folders.len() - 12);
        }
    }
    match &f.reader {
        None => {}
        Some(Ok(u)) => println!("\nreader ({}): {:?} groups {:?}, confidence {:.2}", u.provider, u.intent, u.groups, u.confidence),
        Some(Err(e)) => println!("\nreader: {e}"),
    }
}

fn cmd_find(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    if o.words.is_empty() {
        return usage_error("find needs a query");
    }
    let query = o.words.join(" ");
    let root = o.root.unwrap_or_else(home);
    match find(&query, &root, o.ask, o.json) {
        Ok(f) => {
            if o.json {
                println!("{}", serde_json::to_string_pretty(&found_json(&f, &query)).unwrap());
            } else {
                print_found(&f);
            }
            0
        }
        Err(e) => {
            eprintln!("broom: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------- trash

/// Paths the CLI will never move, whatever it's told: the disk root, the
/// system folders, any volume mount, the home folder and anything above it,
/// a Trash folder itself (Finder empties those). Things *inside* ~/Library,
/// /private/tmp or a volume are fine; that's where the junk lives.
fn refuse(path: &Path) -> Option<&'static str> {
    if path.starts_with("/System") || path.starts_with("/usr") || path.starts_with("/bin") || path.starts_with("/sbin") {
        return Some("part of macOS");
    }
    if path.file_name().is_some_and(|n| n == ".Trash" || n == ".Trashes") {
        return Some("a Trash folder (empty it from Finder)");
    }
    // Top-level system and user roots, and anything they sit inside.
    const ROOTS: [&str; 10] = ["/", "/Library", "/Applications", "/Users", "/Volumes", "/private", "/private/var", "/private/etc", "/private/tmp", "/opt"];
    if ROOTS.iter().any(|r| Path::new(r) == path) {
        return Some("a top-level folder");
    }
    let h = home();
    if path == h {
        return Some("your home folder");
    }
    if h.starts_with(path) {
        return Some("a folder above your home folder");
    }
    // The standard folders directly under home: trash things inside them, never the folder.
    const HOME_FOLDERS: [&str; 9] = ["Library", "Desktop", "Documents", "Downloads", "Movies", "Music", "Pictures", "Public", "Applications"];
    if path.parent() == Some(h.as_path()) && path.file_name().is_some_and(|n| HOME_FOLDERS.iter().any(|f| n == *f)) {
        return Some("a standard folder in your home (trash things inside it, not the folder)");
    }
    if path.starts_with("/Volumes") && path.components().count() <= 3 {
        return Some("a volume");
    }
    if path.starts_with("/Applications") && path.components().count() <= 3 {
        return Some("an installed app (drag it to the Trash in Finder)");
    }
    None
}

struct Trashed {
    gone: Vec<PathBuf>,
    failed: Vec<(PathBuf, String)>,
    frees: u64,
}

/// Move each path to the Trash, one at a time so a failure is attributed to its
/// path and the rest still go. `frees` is what the lot would really free,
/// measured before anything moves.
fn trash_paths(paths: &[PathBuf], quiet: bool) -> Trashed {
    let frees = scan::frees_of(paths);
    let mut gone = Vec::new();
    let mut failed = Vec::new();
    for p in paths {
        match crate::trashcan::send(std::slice::from_ref(p)) {
            Ok(()) => {
                if !quiet {
                    eprintln!("trashed {}", tilde(p));
                }
                gone.push(p.clone());
            }
            Err(e) => failed.push((p.clone(), e)),
        }
    }
    Trashed { gone, failed, frees }
}

fn trashed_json(t: &Trashed) -> Value {
    json!({
        "trashed": t.gone,
        "failed": t.failed.iter().map(|(p, e)| json!({ "path": p, "error": e })).collect::<Vec<_>>(),
        "frees_bytes": t.frees,
    })
}

fn cmd_trash(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    if o.words.is_empty() {
        return usage_error("trash needs at least one path");
    }
    let mut paths = Vec::new();
    for w in &o.words {
        let p = PathBuf::from(w);
        let p = match p.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("broom: {}: {e}", p.display());
                return 1;
            }
        };
        if let Some(why) = refuse(&p) {
            eprintln!("broom: refusing to trash {}: {why}", p.display());
            return 1;
        }
        paths.push(p);
    }
    if !o.yes {
        let frees = scan::frees_of(&paths);
        if o.json {
            println!("{}", serde_json::to_string_pretty(&json!({ "dry_run": true, "paths": paths, "frees_bytes": frees })).unwrap());
        } else {
            for p in &paths {
                println!("would trash {}", tilde(p));
            }
            println!("{} back once the Trash is emptied. Add --yes to do it.", format_size(frees));
        }
        return 0;
    }
    let t = trash_paths(&paths, o.json);
    if o.json {
        println!("{}", serde_json::to_string_pretty(&trashed_json(&t)).unwrap());
    } else {
        println!("moved {} of {} to the Trash; {} back once it's emptied.", t.gone.len(), paths.len(), format_size(t.frees));
        for (p, e) in &t.failed {
            println!("couldn't move {}: {e}", tilde(p));
        }
    }
    if t.failed.is_empty() { 0 } else { 1 }
}

// ---------------------------------------------------------------- clean

fn cmd_clean(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    if o.words.is_empty() {
        return usage_error("clean needs a query");
    }
    let query = o.words.join(" ");
    let root = o.root.unwrap_or_else(home);
    let f = match find(&query, &root, o.ask, o.json) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("broom: {e}");
            return 1;
        }
    };
    let tree = &f.scanned.tree;
    // Which groups go: precise matches only (a name match, never a substring or
    // a kind), safe unless --all. The reader, when asked, may narrow that to
    // the groups it understood the question to mean.
    let mut selected: Vec<usize> = f
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.source.auto_tick() && (o.all || g.safety == Safety::Safe))
        .map(|(i, _)| i)
        .collect();
    if let Some(Ok(u)) = &f.reader {
        if u.intent == ask::Intent::Delete && !u.groups.is_empty() {
            selected.retain(|i| u.groups.contains(i));
        }
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for &i in &selected {
        for &n in &f.folders[i] {
            let p = tree.path_of(n);
            if refuse(&p).is_none() && !paths.iter().any(|q| p.starts_with(q)) {
                paths.push(p);
            }
        }
    }
    if !o.yes {
        let frees = if paths.is_empty() { 0 } else { scan::frees_of(&paths) };
        if o.json {
            let mut out = found_json(&f, &query);
            out["dry_run"] = json!(true);
            out["selected"] = json!(selected);
            out["paths"] = json!(paths);
            out["frees_bytes"] = json!(frees);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            print_found(&f);
            println!();
            if paths.is_empty() {
                println!("nothing to clean: no precise, {} group matched. `broom find` shows everything that did.", if o.all { "collectable" } else { "safe" });
            } else {
                println!("would trash {} folder{} from {:?}: {} back once the Trash is emptied. Add --yes to do it{}.", paths.len(), if paths.len() == 1 { "" } else { "s" }, selected, format_size(frees), if o.all { "" } else { ", --all to include review groups" });
            }
        }
        return 0;
    }
    if paths.is_empty() {
        if o.json {
            let mut out = found_json(&f, &query);
            out["selected"] = json!(selected);
            out["trashed"] = json!([]);
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
        } else {
            println!("nothing to clean");
        }
        return 0;
    }
    let t = trash_paths(&paths, o.json);
    if o.json {
        let mut out = found_json(&f, &query);
        out["selected"] = json!(selected);
        let tj = trashed_json(&t);
        out["trashed"] = tj["trashed"].clone();
        out["failed"] = tj["failed"].clone();
        out["frees_bytes"] = tj["frees_bytes"].clone();
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("moved {} of {} to the Trash; {} back once it's emptied.", t.gone.len(), paths.len(), format_size(t.frees));
        for (p, e) in &t.failed {
            println!("couldn't move {}: {e}", tilde(p));
        }
    }
    if t.failed.is_empty() { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_words_from_options() {
        let o = parse(&["node".into(), "modules".into(), "--root".into(), "/x".into(), "--json".into(), "--yes".into()]).unwrap();
        assert_eq!(o.words, vec!["node", "modules"]);
        assert_eq!(o.root.as_deref(), Some(Path::new("/x")));
        assert!(o.json && o.yes && !o.all && !o.ask);
        assert!(parse(&["--bogus".into()]).is_err());
        assert!(parse(&["--root".into()]).is_err());
    }

    #[test]
    fn refuses_the_roots_and_home_but_not_the_junk_inside() {
        let h = home();
        for p in ["/", "/System/Library/Caches", "/usr/local", "/Library", "/Applications", "/Users", "/Volumes", "/Volumes/Data", "/private", "/private/tmp", "/opt"] {
            assert!(refuse(Path::new(p)).is_some(), "{p} should be refused");
        }
        assert!(refuse(&h).is_some());
        assert!(refuse(h.parent().unwrap()).is_some());
        for f in ["Library", "Documents", "Downloads", "Desktop"] {
            assert!(refuse(&h.join(f)).is_some(), "~/{f} should be refused");
        }
        assert!(refuse(&h.join(".Trash")).is_some());
        for p in [h.join("Library/Caches"), h.join("Projects/app/node_modules"), h.join("Downloads/old.dmg"), PathBuf::from("/private/tmp/build"), PathBuf::from("/Volumes/Data/junk"), PathBuf::from("/Applications/X.app/Contents")] {
            assert!(refuse(&p).is_none(), "{} should be allowed", p.display());
        }
    }

    #[test]
    fn run_ignores_app_arguments() {
        assert!(run(&["broom".into()]).is_none());
        assert!(run(&["broom".into(), "/Users".into()]).is_none());
        assert!(run(&["broom".into(), "--bench-scan".into()]).is_none());
        assert_eq!(run(&["broom".into(), "find".into()]), Some(2));
        assert_eq!(run(&["broom".into(), "trash".into()]), Some(2));
        assert_eq!(run(&["broom".into(), "scan".into(), "a".into(), "b".into()]), Some(2));
    }
}
