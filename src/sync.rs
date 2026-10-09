//! Two-way folder sync with a peer: a three-way merge against the *base* (what
//! both sides looked like after the last successful pass), so edits, new
//! files, deletions and conflicts are told apart. Conflicts keep both copies.

use crate::client;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SKIP: &[&str] = &["node_modules", "target", ".git"];

/// What a file looked like: size and modification time (seconds).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Seen {
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Base {
    local: BTreeMap<String, Seen>,
    remote: BTreeMap<String, Seen>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Action {
    Push(String),
    Pull(String),
    DeleteRemote(String),
    DeleteLocal(String),
    /// Both changed differently: keep local, save remote beside it.
    Conflict(String),
}

/// Decide what to do for every path. `same_content(path)` is asked only when
/// both sides changed, to tell an identical edit from a real conflict.
pub fn plan(
    base: (&BTreeMap<String, Seen>, &BTreeMap<String, Seen>),
    local: &BTreeMap<String, Seen>,
    remote: &BTreeMap<String, Seen>,
    same_content: &mut dyn FnMut(&str) -> bool,
) -> Vec<Action> {
    let (base_l, base_r) = base;
    let paths: BTreeSet<&String> = base_l
        .keys()
        .chain(base_r.keys())
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    let mut out = Vec::new();
    for p in paths {
        let (l, r, bl, br) = (local.get(p), remote.get(p), base_l.get(p), base_r.get(p));
        let l_changed = l != bl;
        let r_changed = r != br;
        match (l, r) {
            (Some(_), Some(_)) => {
                if l_changed && r_changed {
                    if !same_content(p) {
                        out.push(Action::Conflict(p.clone()));
                    }
                } else if l_changed {
                    out.push(Action::Push(p.clone()));
                } else if r_changed {
                    out.push(Action::Pull(p.clone()));
                }
            }
            (Some(_), None) => {
                // Gone remotely: delete here only if it was synced and unchanged here.
                if br.is_some() && !l_changed {
                    out.push(Action::DeleteLocal(p.clone()));
                } else {
                    out.push(Action::Push(p.clone()));
                }
            }
            (None, Some(_)) => {
                if bl.is_some() && !r_changed {
                    out.push(Action::DeleteRemote(p.clone()));
                } else {
                    out.push(Action::Pull(p.clone()));
                }
            }
            (None, None) => {}
        }
    }
    out
}

fn local_index(root: &Path) -> anyhow::Result<BTreeMap<String, Seen>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.')
                || SKIP.contains(&name.as_str())
                || name.ends_with(crate::xfer::PART)
            {
                continue;
            }
            let p = e.path();
            let m = e.metadata()?;
            if m.is_dir() {
                stack.push(p);
            } else if m.is_file() {
                let rel = p.strip_prefix(root)?.to_string_lossy().into_owned();
                out.insert(
                    rel,
                    Seen {
                        size: m.len(),
                        mtime: mtime(&m),
                    },
                );
            }
        }
    }
    Ok(out)
}

fn mtime(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn remote_index(peer: &str, root: &str) -> anyhow::Result<BTreeMap<String, Seen>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let path = if rel.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{rel}")
        };
        let v = client::files_list(peer, &path)?;
        for e in v["entries"].as_array().into_iter().flatten() {
            // The peer names entries; each must be a plain name in this folder,
            // or a local path built from it could point anywhere.
            let name = crate::xfer::plain_name(e["name"].as_str().unwrap_or(""))?;
            if name.starts_with('.') || SKIP.contains(&name) || name.ends_with(crate::xfer::PART) {
                continue;
            }
            let child = if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            };
            if e["dir"].as_bool() == Some(true) {
                stack.push(child);
            } else {
                out.insert(
                    child,
                    Seen {
                        size: e["size"].as_u64().unwrap_or(0),
                        mtime: e["mtime"].as_u64().unwrap_or(0),
                    },
                );
            }
        }
    }
    Ok(out)
}

fn state_file(peer: &str, local: &Path, remote: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = Sha256::digest(format!("{}\0{remote}", local.display()).as_bytes());
    let short: String = key.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/state/omaspace/sync")
        .join(format!("{peer}-{short}.json"))
}

/// `name.ext` → `name (conflict from peer).ext`
fn conflict_name(rel: &str, peer: &str) -> String {
    let p = Path::new(rel);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let name = format!("{stem} (conflict from {peer}){ext}");
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => format!("{}/{name}", d.display()),
        None => name,
    }
}

/// One sync pass. Returns how many actions ran.
pub fn pass(
    peer: &str,
    local_root: &Path,
    remote_root: &str,
    quiet: bool,
) -> anyhow::Result<usize> {
    pass_with(peer, local_root, remote_root, &mut |line| {
        if !quiet {
            println!("{line}");
        }
    })
}

/// One sync pass, each change described to `say`.
fn pass_with(
    peer: &str,
    local_root: &Path,
    remote_root: &str,
    say: &mut dyn FnMut(String),
) -> anyhow::Result<usize> {
    std::fs::create_dir_all(local_root)?;
    let state = state_file(peer, local_root, remote_root);
    let base: Base = std::fs::read(&state)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let stat = client::files_stat(peer, remote_root)?;
    if stat["exists"].as_bool() != Some(true) {
        post(peer, "mkdir", remote_root)?;
    }
    let local = local_index(local_root)?;
    let remote = remote_index(peer, remote_root)?;
    let mut same = |rel: &str| {
        let lh = crate::xfer::hash_file(&local_root.join(rel)).unwrap_or_default();
        let rh = client::get(
            peer,
            &format!(
                "/v1/files/hash?path={}",
                client::enc(&format!("{remote_root}/{rel}"))
            ),
        )
        .ok()
        .and_then(|v| v["sha256"].as_str().map(String::from))
        .unwrap_or_default();
        !lh.is_empty() && lh == rh
    };
    let actions = plan((&base.local, &base.remote), &local, &remote, &mut same);
    for a in &actions {
        match a {
            Action::Push(rel) => {
                client::put_file(
                    peer,
                    &local_root.join(rel),
                    &format!("{remote_root}/{rel}"),
                    true,
                    &mut |_, _| {},
                )
                .with_context(|| format!("sending {rel}"))?;
                say(format!("→ {rel}"));
            }
            Action::Pull(rel) => {
                let dest = local_root.join(rel);
                if dest.exists() {
                    std::fs::remove_file(&dest)?;
                }
                client::get_file(peer, &format!("{remote_root}/{rel}"), &dest, &mut |_, _| {})
                    .with_context(|| format!("fetching {rel}"))?;
                say(format!("← {rel}"));
            }
            Action::DeleteRemote(rel) => {
                post(peer, "delete", &format!("{remote_root}/{rel}"))?;
                say(format!("✕ {rel} (on {peer})"));
            }
            Action::DeleteLocal(rel) => {
                std::fs::remove_file(local_root.join(rel))?;
                say(format!("✕ {rel} (here)"));
            }
            Action::Conflict(rel) => {
                let keep = local_root.join(conflict_name(rel, peer));
                client::get_file(peer, &format!("{remote_root}/{rel}"), &keep, &mut |_, _| {})?;
                client::put_file(
                    peer,
                    &local_root.join(rel),
                    &format!("{remote_root}/{rel}"),
                    true,
                    &mut |_, _| {},
                )?;
                client::put_file(
                    peer,
                    &keep,
                    &format!("{remote_root}/{}", conflict_name(rel, peer)),
                    true,
                    &mut |_, _| {},
                )?;
                say(format!(
                    "! {rel}: changed on both sides; {peer}'s copy kept as {}",
                    conflict_name(rel, peer)
                ));
            }
        }
    }
    // New base: re-read both sides after applying.
    let new = Base {
        local: local_index(local_root)?,
        remote: remote_index(peer, remote_root)?,
    };
    std::fs::create_dir_all(state.parent().unwrap())?;
    std::fs::write(&state, serde_json::to_vec(&new)?)?;
    Ok(actions.len())
}

fn post(peer: &str, what: &str, path: &str) -> anyhow::Result<()> {
    client::post_empty(
        peer,
        &format!("/v1/files/{what}?path={}", client::enc(path)),
    )?;
    Ok(())
}

// ---- folders kept in sync, run by the daemon ------------------------------

/// A folder kept the same on this machine and a peer, until removed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Folder {
    pub peer: String,
    /// Absolute path here.
    pub local: PathBuf,
    /// `~/…` on the peer.
    pub remote: String,
}

/// How a folder's syncing is going (written after every pass).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    /// When the last pass finished without an error (Unix seconds).
    pub last_ok: u64,
    pub last_error: Option<String>,
    /// The most recent changes, newest first (at most 20).
    pub recent: Vec<String>,
}

fn state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".local/state/omaspace")
}

fn folders_path() -> PathBuf {
    state_dir().join("sync-folders.json")
}

pub fn folders() -> Vec<Folder> {
    std::fs::read(folders_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_folders(list: &[Folder]) -> anyhow::Result<()> {
    std::fs::create_dir_all(state_dir())?;
    let tmp = folders_path().with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
    std::fs::rename(tmp, folders_path())?;
    Ok(())
}

fn status_path(f: &Folder) -> PathBuf {
    state_file(&f.peer, &f.local, &f.remote).with_extension("status.json")
}

pub fn status(f: &Folder) -> Status {
    std::fs::read(status_path(f))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One pass for a saved folder, recording how it went.
fn pass_recorded(f: &Folder) -> anyhow::Result<usize> {
    let mut st = status(f);
    let mut changes = Vec::new();
    let result = pass_with(&f.peer, &f.local, &f.remote, &mut |line| changes.push(line));
    match &result {
        Ok(_) => {
            st.last_ok = now();
            st.last_error = None;
        }
        Err(e) => st.last_error = Some(format!("{e:#}")),
    }
    if !changes.is_empty() {
        let stamp = now();
        let mut fresh: Vec<String> = changes
            .into_iter()
            .map(|c| format!("{stamp} {c}"))
            .rev()
            .collect();
        fresh.extend(st.recent);
        fresh.truncate(20);
        st.recent = fresh;
    }
    std::fs::create_dir_all(status_path(f).parent().unwrap())?;
    std::fs::write(status_path(f), serde_json::to_vec(&st)?)?;
    result
}

/// The daemon's sync loop: every saved folder, every few seconds, for as long
/// as it runs, so folders keep syncing across reboots. A peer that's offline
/// is retried with backoff (up to a minute) rather than every pass.
pub fn run_saved() {
    use std::collections::HashMap;
    let mut wait: HashMap<(String, PathBuf), (u32, std::time::Instant)> = HashMap::new();
    loop {
        for f in folders() {
            let key = (f.peer.clone(), f.local.clone());
            if let Some((_, until)) = wait.get(&key)
                && std::time::Instant::now() < *until
            {
                continue;
            }
            match pass_recorded(&f) {
                Ok(_) => {
                    wait.remove(&key);
                }
                Err(e) => {
                    let fails = wait.get(&key).map_or(0, |(n, _)| *n) + 1;
                    let secs = (3u64 << fails.min(5)).min(60);
                    if fails == 1 {
                        eprintln!("sync {} <-> {}: {e:#}", f.local.display(), f.peer);
                    }
                    wait.insert(
                        key,
                        (
                            fails,
                            std::time::Instant::now() + std::time::Duration::from_secs(secs),
                        ),
                    );
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

fn absolute(dir: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(dir);
    Ok(if p.is_absolute() {
        p
    } else if let Some(rest) = dir.strip_prefix("~/") {
        state_dir()
            .parent()
            .and_then(Path::parent)
            .map(|h| h.join(rest))
            .unwrap_or(p)
    } else {
        std::env::current_dir()?.join(p)
    })
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn ago(t: u64) -> String {
    if t == 0 {
        return "never".into();
    }
    let s = now().saturating_sub(t);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

const USAGE: &str = "usage:
  omaspace sync add <peer> <folder> [remote folder]   keep a folder the same on both, from now on
  omaspace sync list                                  folders kept in sync, and how they're doing
  omaspace sync remove <folder> [peer]                stop syncing it (both copies stay)
  omaspace sync <peer> <folder> [remote folder] [--once]   sync in the foreground (Ctrl-C to stop)";

/// `omaspace sync …`
pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("add") => {
            let peer = args.get(1).context(USAGE)?;
            let local = absolute(args.get(2).context(USAGE)?)?;
            std::fs::create_dir_all(&local)?;
            let local = local.canonicalize()?;
            let remote = args
                .get(3)
                .cloned()
                .unwrap_or_else(|| crate::snapshot::to_portable(&local, &home()));
            anyhow::ensure!(
                remote.starts_with("~/"),
                "the folder must be inside your home folder (or name the remote one: ~/…)"
            );
            anyhow::ensure!(
                client::peers()?
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(peer)),
                "{peer} isn't one of your machines running omaspace (see omaspace peers)"
            );
            let f = Folder {
                peer: peer.clone(),
                local,
                remote,
            };
            // A first pass now, so mistakes show here, not later in a log.
            let n = pass_recorded(&f)?;
            let mut list = folders();
            list.retain(|x| !(x.peer == f.peer && x.local == f.local));
            list.push(f.clone());
            save_folders(&list)?;
            println!(
                "syncing {} ⇄ {}:{} ({n} change(s) so far); it keeps syncing in the background, also after a restart",
                f.local.display(),
                f.peer,
                f.remote
            );
            Ok(())
        }
        Some("list") | Some("status") => {
            let list = folders();
            if list.is_empty() {
                println!("no folders kept in sync (add one: omaspace sync add <peer> <folder>)");
            }
            for f in &list {
                let st = status(f);
                let state = match &st.last_error {
                    Some(e) => format!("error: {e}"),
                    None => format!("synced {}", ago(st.last_ok)),
                };
                println!(
                    "{}  ⇄  {}:{}   {state}",
                    crate::snapshot::to_portable(&f.local, &home()),
                    f.peer,
                    f.remote
                );
            }
            Ok(())
        }
        Some("remove") => {
            let local = absolute(args.get(1).context(USAGE)?)?;
            let local = local.canonicalize().unwrap_or(local);
            let peer = args.get(2);
            let mut list = folders();
            let before = list.len();
            list.retain(|f| !(f.local == local && peer.is_none_or(|p| &f.peer == p)));
            anyhow::ensure!(
                list.len() < before,
                "{} isn't kept in sync",
                local.display()
            );
            save_folders(&list)?;
            println!(
                "stopped syncing {} (the files stay on both machines)",
                local.display()
            );
            Ok(())
        }
        Some("help") | Some("--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        _ => run_foreground(args),
    }
}

/// `omaspace sync <peer> <local dir> [remote dir] [--once]`
fn run_foreground(args: &[String]) -> anyhow::Result<()> {
    let once = args.iter().any(|a| a == "--once");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let peer = pos.first().context(USAGE)?;
    let local = absolute(pos.get(1).context(USAGE)?)?;
    let remote = pos
        .get(2)
        .map(|s| s.to_string())
        .unwrap_or_else(|| crate::snapshot::to_portable(&local, &home()));
    anyhow::ensure!(
        remote.starts_with("~/"),
        "remote folder must be inside the peer's home (~/…)"
    );
    println!(
        "syncing {} ⇄ {peer}:{remote}{}",
        local.display(),
        if once { "" } else { " (Ctrl-C to stop)" }
    );
    loop {
        let n = pass(peer, &local, &remote, false)?;
        if once {
            println!("{n} change(s)");
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(entries: &[(&str, u64)]) -> BTreeMap<String, Seen> {
        entries
            .iter()
            .map(|(p, t)| (p.to_string(), Seen { size: 1, mtime: *t }))
            .collect()
    }

    #[test]
    fn three_way_rule_tells_edits_deletes_and_conflicts_apart() {
        let base_l = m(&[("a", 1), ("b", 1), ("c", 1), ("d", 1), ("e", 1)]);
        let base_r = base_l.clone();
        let local = m(&[("a", 2), ("b", 1), ("d", 5), ("e", 7), ("new-here", 1)]); // a edited, c deleted
        let remote = m(&[("a", 1), ("b", 3), ("c", 1), ("e", 9), ("new-there", 1)]); // b edited, d deleted
        let mut never_same = |_: &str| false;
        let actions = plan((&base_l, &base_r), &local, &remote, &mut never_same);
        assert!(actions.contains(&Action::Push("a".into())));
        assert!(actions.contains(&Action::Pull("b".into())));
        assert!(actions.contains(&Action::DeleteRemote("c".into())));
        assert!(
            actions.contains(&Action::Push("d".into())),
            "edited here, deleted there: keep the edit"
        );
        assert!(actions.contains(&Action::Conflict("e".into())));
        assert!(actions.contains(&Action::Push("new-here".into())));
        assert!(actions.contains(&Action::Pull("new-there".into())));
    }

    #[test]
    fn first_sync_never_deletes() {
        let empty = BTreeMap::new();
        let local = m(&[("x", 1)]);
        let remote = m(&[("y", 1)]);
        let actions = plan((&empty, &empty), &local, &remote, &mut |_| false);
        assert_eq!(
            actions,
            vec![Action::Push("x".into()), Action::Pull("y".into())]
        );
    }

    #[test]
    fn identical_edits_on_both_sides_are_not_a_conflict() {
        let base = m(&[("a", 1)]);
        let both = m(&[("a", 2)]);
        assert!(plan((&base, &base), &both, &both, &mut |_| true).is_empty());
    }

    #[test]
    fn conflict_copies_are_named_beside_the_original() {
        assert_eq!(
            conflict_name("notes/todo.md", "alpha"),
            "notes/todo (conflict from alpha).md"
        );
        assert_eq!(
            conflict_name("Makefile", "bravo"),
            "Makefile (conflict from bravo)"
        );
    }
}
