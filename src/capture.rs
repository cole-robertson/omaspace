//! Build a snapshot of this Omarchy desktop from Hyprland, `/proc` and
//! Chromium's session file. Nothing secret is read: no cookies, no keyring,
//! no environment variables.

use crate::hypr::{self, Client};
use crate::omarchy::{self, Browser};
use crate::snapshot::{self, Content, Snapshot, Window, Workspace};
use crate::snss;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub enum Scope {
    Active,
    One(i64),
    All,
    /// One window, by Hyprland address (what the Spaces panel drops).
    Window(String),
}

pub struct Skipped {
    pub title: String,
    pub reason: String,
}

pub fn capture(
    scope: Scope,
    source: &str,
    home: &Path,
) -> anyhow::Result<(Snapshot, Vec<Skipped>)> {
    capture_with(scope, source, home, &crate::profile::policy_allowed(home))
}

/// Capture, plus the browser profile data for the sites in view. `allowed`
/// lists the sensitive items (cookies, …) allowed for this transfer.
pub fn capture_with(
    scope: Scope,
    source: &str,
    home: &Path,
    allowed: &[String],
) -> anyhow::Result<(Snapshot, Vec<Skipped>)> {
    let (wanted, window) = match scope {
        Scope::Active => (Some(hypr::active_workspace()?), None),
        Scope::One(id) => (Some(id), None),
        Scope::All => (None, None),
        Scope::Window(address) => (None, Some(address)),
    };
    let clients: Vec<Client> = hypr::clients()?
        .into_iter()
        .filter(|c| c.mapped && c.workspace.id > 0)
        .filter(|c| wanted.is_none_or(|id| c.workspace.id == id))
        .filter(|c| window.as_ref().is_none_or(|a| &c.address == a))
        .collect();
    if let Some(address) = &window {
        anyhow::ensure!(!clients.is_empty(), "no window {address} on this desktop");
    }

    let browsers = omarchy::browsers(home);
    let mut browser_windows = BrowserWindows::default();
    let mut skipped = Vec::new();
    let mut workspaces: BTreeMap<i64, Workspace> = BTreeMap::new();
    for client in &clients {
        let content = match classify(client, home, &browsers, &mut browser_windows) {
            Ok(content) => content,
            Err(reason) => {
                skipped.push(Skipped {
                    title: client.title.clone(),
                    reason,
                });
                continue;
            }
        };
        workspaces
            .entry(client.workspace.id)
            .or_insert_with(|| Workspace {
                id: client.workspace.id,
                name: client.workspace.name.clone(),
                windows: Vec::new(),
            })
            .windows
            .push(Window {
                class: client.class.clone(),
                title: client.title.clone(),
                floating: client.floating,
                at: client.at,
                size: client.size,
                fullscreen: client.fullscreen,
                content,
            });
    }
    // Profile data, once per browser profile, for the URLs open in it.
    let mut browser_data = std::collections::BTreeMap::new();
    for client in &clients {
        let Some(browser) = omarchy::browser_for_class(&browsers, &client.class.to_lowercase())
        else {
            continue;
        };
        if browser_data.contains_key(&browser.class) {
            continue;
        }
        let urls: Vec<String> = workspaces
            .values()
            .flat_map(|w| &w.windows)
            .filter_map(|w| match &w.content {
                Content::Browser {
                    browser: b, urls, ..
                } if b == &browser.class => Some(urls.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        if urls.is_empty() {
            continue;
        }
        let pid = u32::try_from(client.pid).unwrap_or(0);
        let profile = browser_data_dir(pid, home, browser).join("Default");
        match crate::profile::capture(&profile, &urls, allowed) {
            Ok(data) => {
                browser_data.insert(browser.class.clone(), data);
            }
            Err(e) => skipped.push(Skipped {
                title: format!("{} profile data", browser.class),
                reason: e.to_string(),
            }),
        }
    }
    let taken_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    Ok((
        Snapshot {
            format: snapshot::FORMAT.into(),
            id: format!("{source}-{taken_at}"),
            source: source.into(),
            taken_at,
            workspaces: workspaces.into_values().collect(),
            browser_data,
        },
        skipped,
    ))
}

fn classify(
    client: &Client,
    home: &Path,
    browsers: &[Browser],
    windows: &mut BrowserWindows,
) -> Result<Content, String> {
    let class = client.class.to_lowercase();
    let pid = u32::try_from(client.pid).map_err(|_| "no process".to_string())?;
    // Omarchy web apps run as `chrome-<host>__-Default` windows; their
    // .desktop file names the URL.
    if let Some((desktop_id, url)) = omarchy::webapp_for_class(&client.class, home) {
        return Ok(Content::WebApp { desktop_id, url });
    }
    if let Some(browser) = omarchy::browser_for_class(browsers, &class) {
        let data_dir = browser_data_dir(pid, home, browser);
        let (urls, active) = windows
            .take(&data_dir, &client.title)
            .ok_or_else(|| format!("no open window in the session at {}", data_dir.display()))?;
        return Ok(Content::Browser {
            browser: browser.class.clone(),
            urls,
            active,
        });
    }
    if omarchy::TERMINAL_CLASSES.contains(&class.as_str()) {
        return Ok(terminal(pid, home));
    }
    let desktop_id = desktop_id_for(&client.class, &client.initial_class, pid, home)
        .ok_or_else(|| format!("{} has no installed .desktop launcher", client.class))?;
    Ok(Content::App { desktop_id })
}

/// The interesting process inside a terminal: the deepest descendant that is
/// a shell, an editor or a tmux client.
fn terminal(pid: u32, home: &Path) -> Content {
    let mut cwd = proc_cwd(pid).unwrap_or_else(|| home.to_path_buf());
    let mut tmux_session = None;
    for child in descendants(pid) {
        let argv = proc_argv(child);
        let Some(bin) = argv.first().map(|a| basename(a)) else {
            continue;
        };
        if bin == "tmux" {
            tmux_session = tmux_client_session(child);
        } else if omarchy::TERMINAL_EDITORS.contains(&bin) {
            let dir = proc_cwd(child).unwrap_or_else(|| cwd.clone());
            let files = argv[1..]
                .iter()
                .filter(|a| !a.starts_with('-') && !a.starts_with('+'))
                .map(|a| snapshot::to_portable(&dir.join(a), home))
                .collect();
            return Content::Editor {
                editor: bin.to_string(),
                cwd: snapshot::to_portable(&dir, home),
                files,
            };
        } else if omarchy::SHELLS.contains(&bin)
            && let Some(dir) = proc_cwd(child)
        {
            cwd = dir;
        }
    }
    Content::Terminal {
        cwd: snapshot::to_portable(&cwd, home),
        tmux_session,
    }
}

/// tmux names the session a client is attached to; ask tmux by client pid.
fn tmux_client_session(client_pid: u32) -> Option<String> {
    let out = std::process::Command::new("tmux")
        .args(["list-clients", "-F", "#{client_pid} #{session_name}"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let (pid, name) = line.split_once(' ')?;
            (pid == client_pid.to_string()).then(|| name.to_string())
        })
}

fn descendants(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut queue = vec![pid];
    while let Some(p) = queue.pop() {
        for task in fs::read_dir(format!("/proc/{p}/task"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let children = fs::read_to_string(task.path().join("children")).unwrap_or_default();
            for child in children.split_whitespace().filter_map(|c| c.parse().ok()) {
                out.push(child);
                queue.push(child);
            }
        }
    }
    out
}

fn proc_cwd(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

fn proc_argv(pid: u32) -> Vec<String> {
    fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// An installed `.desktop` file for this window: by class / initial class, or
/// one whose `Exec` binary is the window's executable.
fn desktop_id_for(class: &str, initial_class: &str, pid: u32, home: &Path) -> Option<String> {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok();
    let exe_name = exe
        .as_ref()
        .and_then(|e| e.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let dirs = omarchy::application_dirs(home);
    for name in [class, initial_class] {
        if name.is_empty() {
            continue;
        }
        for dir in &dirs {
            for candidate in [
                format!("{name}.desktop"),
                format!("{}.desktop", name.to_lowercase()),
            ] {
                if dir.join(&candidate).is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // A browser's own binary matches every app's .desktop; only browsers'
    // windows are tabs, so don't map them to an app by executable.
    let exe_name =
        exe_name.filter(|n| !n.contains("chrom") && !n.contains("brave") && !n.contains("edge"))?;
    for dir in &dirs {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
            let exec_bin = text
                .lines()
                .find_map(|l| l.strip_prefix("Exec="))
                .and_then(|e| e.split_whitespace().next())
                .map(|b| basename(b).to_string());
            if exec_bin.as_deref() == Some(exe_name.as_str()) {
                return entry.file_name().into_string().ok();
            }
        }
    }
    None
}

/// A browser window's profile data dir: its process's `--user-data-dir`, else
/// that browser's default under `~/.config`. Chromium rewrites its `/proc`
/// cmdline into one space-separated string, so split on whitespace too.
fn browser_data_dir(pid: u32, home: &Path, browser: &Browser) -> PathBuf {
    proc_argv(pid)
        .iter()
        .flat_map(|a| a.split_whitespace())
        .find_map(|a| a.strip_prefix("--user-data-dir=").map(PathBuf::from))
        .unwrap_or_else(|| home.join(".config").join(&browser.config_dir))
}

/// When the browser using `data_dir` started (its main process), if running.
fn browser_started(data_dir: &Path) -> Option<std::time::SystemTime> {
    let lock = fs::read_link(data_dir.join("SingletonLock")).ok()?;
    let pid = lock.to_str()?.rsplit_once('-')?.1.parse::<u32>().ok()?;
    fs::metadata(format!("/proc/{pid}")).ok()?.modified().ok()
}

/// Chromium windows from each data dir's session file, handed out to Hyprland
/// windows by matching each window's title to its active tab, then in order.
#[derive(Default)]
struct BrowserWindows {
    by_dir: BTreeMap<PathBuf, Vec<snss::Window>>,
    /// Session window ids already handed out, and dirs already re-read.
    taken: BTreeMap<PathBuf, Vec<i32>>,
    reloaded: Vec<PathBuf>,
}

impl BrowserWindows {
    fn load(data_dir: &Path) -> Vec<snss::Window> {
        let dir = data_dir.join("Default/Sessions");
        let mut files: Vec<_> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("Session_"))
            .collect();
        files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        // Chromium holds the file open and appends; read a copy-in-memory.
        // The newest file is the running session, except in the seconds
        // after a (re)start, before Chromium has written its new one: then
        // the newest is the previous run's and lists windows that are gone.
        // A file written before this browser process started is stale.
        let started = browser_started(data_dir);
        for e in files {
            let fresh = match (started, e.metadata().and_then(|m| m.modified()).ok()) {
                // /proc's time trails the real start by up to a second.
                (Some(s), Some(m)) => m + std::time::Duration::from_secs(2) >= s,
                _ => true,
            };
            if !fresh {
                continue;
            }
            if let Some(windows) = fs::read(e.path())
                .ok()
                .and_then(|data| snss::parse(&data).ok())
            {
                return windows;
            }
        }
        Vec::new()
    }

    fn take(&mut self, data_dir: &Path, title: &str) -> Option<(Vec<String>, usize)> {
        let key = data_dir.to_path_buf();
        let mut best = self.best_match(data_dir, title);
        // Chromium saves its session ~2.5s after a change, so a window opened
        // just now isn't in the file yet: re-read once before falling back to
        // file order, which would hand this window another window's tabs.
        if best.is_none() && !self.reloaded.contains(&key) {
            self.reloaded.push(key.clone());
            std::thread::sleep(std::time::Duration::from_millis(3000));
            let taken = self.taken.get(&key).cloned().unwrap_or_default();
            let fresh: Vec<_> = Self::load(data_dir)
                .into_iter()
                .filter(|w| !taken.contains(&w.id))
                .collect();
            self.by_dir.insert(key.clone(), fresh);
            best = self.best_match(data_dir, title);
        }
        let windows = self.by_dir.get_mut(&key)?;
        if windows.is_empty() {
            return None;
        }
        let window = windows.remove(best.unwrap_or(0));
        self.taken.entry(key).or_default().push(window.id);
        Some((window.urls, window.active))
    }

    fn best_match(&mut self, data_dir: &Path, title: &str) -> Option<usize> {
        let windows = self
            .by_dir
            .entry(data_dir.to_path_buf())
            .or_insert_with(|| Self::load(data_dir));
        // On a tie (two windows showing the same page) prefer the newest:
        // a session file can still list a window closed seconds ago, and it
        // comes earlier in the file than one just opened. `max_by_key` keeps
        // the last of equal maxima, so iterate oldest to newest.
        (0..windows.len())
            .filter(|&i| title_matches(title, &windows[i]))
            .max_by_key(|&i| (title_score(title, &windows[i]), windows[i].id))
    }
}

/// The session file has URLs but not page titles, and Hyprland has titles but
/// not URLs; match on the words they share (`Example Domain` ↔ `example.org`).
/// A tie or no overlap falls back to session-file order.
fn title_matches(title: &str, window: &snss::Window) -> bool {
    title_score(title, window) > 0
}

fn title_score(title: &str, window: &snss::Window) -> usize {
    let page = title.trim_end_matches(" - Chromium").to_lowercase();
    let Some(url) = window.urls.get(window.active) else {
        return 0;
    };
    let url = url.to_lowercase();
    if url.starts_with("chrome://newtab") {
        return usize::from(page.starts_with("new tab"));
    }
    page.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4)
        .filter(|w| url.contains(w))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(url: &str) -> snss::Window {
        snss::Window {
            id: 0,
            urls: vec![url.into()],
            active: 0,
        }
    }

    #[test]
    fn a_tie_goes_to_the_newest_window_not_a_just_closed_one() {
        let mut browsers = BrowserWindows::default();
        let dir = PathBuf::from("/x");
        let w = |id, urls: &[&str]| snss::Window {
            id,
            urls: urls.iter().map(|u| u.to_string()).collect(),
            active: 0,
        };
        browsers.by_dir.insert(
            dir.clone(),
            vec![
                w(
                    10,
                    &[
                        "https://example.org/",
                        "https://www.iana.org/help/example-domains",
                    ],
                ), // closed a moment ago
                w(12, &["https://example.org/", "https://www.rfc-editor.org/"]), // just opened
            ],
        );
        assert_eq!(
            browsers.take(&dir, "Example Domain - Chromium").unwrap().0[1],
            "https://www.rfc-editor.org/"
        );
    }

    #[test]
    fn hyprland_titles_pick_the_session_window_showing_that_page() {
        let mut browsers = BrowserWindows::default();
        let dir = PathBuf::from("/x");
        browsers.by_dir.insert(
            dir.clone(),
            vec![
                window("https://www.iana.org/help/example-domains"),
                window("chrome://newtab/"),
                window("https://www.rfc-editor.org/"),
            ],
        );
        assert_eq!(
            browsers.take(&dir, "RFC Editor - Chromium").unwrap().0,
            vec!["https://www.rfc-editor.org/"]
        );
        assert_eq!(
            browsers.take(&dir, "New Tab - Chromium").unwrap().0,
            vec!["chrome://newtab/"]
        );
        assert_eq!(
            browsers.take(&dir, "Example Domains - Chromium").unwrap().0[0],
            "https://www.iana.org/help/example-domains"
        );
        assert!(browsers.take(&dir, "anything").is_none());
    }
}
