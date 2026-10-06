//! Browser profile data that travels with a workspace, following cua Spaces'
//! defaults: everything not credential-shaped moves by default; Cookies,
//! Login Data (saved passwords) and History are withheld unless allowed.
//!
//! Scoped to the sites open in the workspace (cua copies the whole profile;
//! omaspace only the sites you were working in, plus `file://` pages).

use crate::{cookies, storage};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The items, named as cua names them in its teleport manifest.
pub const SENSITIVE: &[&str] = &["cookies", "login_data", "history"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileData {
    /// The sites (registrable hosts, or "file") this data is scoped to.
    pub sites: Vec<String>,
    #[serde(default)]
    pub local_storage: Vec<storage::Entry>,
    /// Profile files copied only into a profile that lacks them
    /// (Bookmarks, Preferences): base64 by file name.
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, String>,
    /// Withheld unless allowed (sensitive).
    #[serde(default)]
    pub cookies: Vec<cookies::Cookie>,
    /// Which sensitive items were allowed for this transfer.
    #[serde(default)]
    pub allowed_sensitive: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Policy {
    #[serde(default)]
    allow_sensitive: Vec<String>,
}

/// Sensitive items allowed ahead of time in `~/.config/omaspace/policy.json`
/// (the counterpart of cua's `~/.cua/spaces-teleport-policy.json`).
pub fn policy_allowed(home: &Path) -> Vec<String> {
    std::fs::read(home.join(".config/omaspace/policy.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Policy>(&b).ok())
        .map(|p| {
            p.allow_sensitive
                .into_iter()
                .filter(|i| SENSITIVE.contains(&i.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// `https://api.github.com/x` → `github.com`; `file:///…` → `file`.
pub fn site_of(url: &str) -> Option<String> {
    if url.starts_with("file://") {
        return Some("file".into());
    }
    let host = url
        .split("://")
        .nth(1)?
        .split(['/', ':', '?', '#'])
        .next()?
        .to_lowercase();
    if host.is_empty() || !url.starts_with("http") {
        return None;
    }
    // Registrable domain by the last two labels (three for common second-level
    // public suffixes); good enough to scope cookies to the sites in view.
    let labels: Vec<&str> = host.split('.').collect();
    let keep = if labels.len() >= 3
        && ["co", "com", "org", "net", "gov", "ac"].contains(&labels[labels.len() - 2])
        && labels[labels.len() - 1].len() == 2
    {
        3
    } else {
        2
    };
    Some(labels[labels.len().saturating_sub(keep)..].join("."))
}

pub fn sites_of(urls: &[String]) -> Vec<String> {
    urls.iter()
        .filter_map(|u| site_of(u))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Collect the profile data for `urls` from a profile dir (`…/Default`).
pub fn capture(profile: &Path, urls: &[String], allowed: &[String]) -> anyhow::Result<ProfileData> {
    let sites = sites_of(urls);
    let mut data = ProfileData {
        sites: sites.clone(),
        ..Default::default()
    };
    data.local_storage = storage::read(profile, &sites)?;
    for name in ["Bookmarks", "Preferences"] {
        if let Ok(bytes) = std::fs::read(profile.join(name)) {
            data.files.insert(
                name.into(),
                base64::engine::general_purpose::STANDARD.encode(bytes),
            );
        }
    }
    if allowed.iter().any(|i| i == "cookies") {
        data.cookies = cookies::read(profile, &sites, &cookies::Keys::local())?;
        data.allowed_sensitive.push("cookies".into());
    }
    Ok(data)
}

/// Everything an agent's browser needs to be signed in as you: every site's
/// cookies and local storage (not passwords or history), plus Preferences.
/// `sites` holds "*", which `apply_all` (and only it) accepts.
pub fn capture_all(profile: &Path) -> anyhow::Result<ProfileData> {
    let all = vec![ALL_SITES.to_string()];
    let mut data = ProfileData {
        sites: all.clone(),
        ..Default::default()
    };
    data.local_storage = storage::read(profile, &all)?;
    if let Ok(bytes) = std::fs::read(profile.join("Preferences")) {
        data.files.insert(
            "Preferences".into(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
    }
    data.cookies = cookies::read(profile, &all, &cookies::Keys::local())?;
    data.allowed_sensitive.push("cookies".into());
    Ok(data)
}

/// `apply` for whole-profile data from `capture_all` (this machine to itself).
/// A new profile gets an empty Cookies database first: Chromium creates its
/// own only on first run, and `cookies::write` needs one to merge into.
pub fn apply_all(profile: &Path, data: &ProfileData) -> anyhow::Result<Applied> {
    anyhow::ensure!(data.sites == [ALL_SITES], "not whole-profile data");
    std::fs::create_dir_all(profile)?;
    if !profile.join("Cookies").is_file() && !data.cookies.is_empty() {
        cookies::create_empty(&profile.join("Cookies"))?;
    }
    apply(profile, data)
}

/// The site filter that matches every site (whole-profile copies only).
pub const ALL_SITES: &str = "*";

#[derive(Debug, Default, Serialize)]
pub struct Applied {
    pub local_storage: usize,
    pub cookies: usize,
    pub files: Vec<String>,
}

/// Write `data` into a profile dir whose browser is closed.
pub fn apply(profile: &Path, data: &ProfileData) -> anyhow::Result<Applied> {
    let mut applied = Applied::default();
    std::fs::create_dir_all(profile)?;
    // Mark the user-data dir as past first run, as a normal launch would.
    if let Some(data_dir) = profile.parent() {
        let marker = data_dir.join("First Run");
        if !marker.exists() {
            std::fs::write(marker, b"")?;
        }
    }
    for (name, content) in &data.files {
        anyhow::ensure!(
            ["Bookmarks", "Preferences"].contains(&name.as_str()),
            "unexpected profile file {name}"
        );
        let path = profile.join(name);
        if !path.exists() {
            std::fs::write(
                &path,
                base64::engine::general_purpose::STANDARD.decode(content)?,
            )?;
            applied.files.push(name.clone());
        }
    }
    if !data.local_storage.is_empty() {
        applied.local_storage = storage::write(profile, &data.local_storage)?;
    }
    if !data.cookies.is_empty() {
        // Only cookies for the declared sites may be written.
        anyhow::ensure!(
            data.cookies
                .iter()
                .all(|c| cookies::host_matches(&c.host_key, &data.sites)),
            "a cookie outside the workspace's sites"
        );
        applied.cookies = cookies::write(profile, &data.cookies, &cookies::Keys::local())?;
    }
    Ok(applied)
}

/// Pids of the browser process(es) using `data_dir` (main process only).
pub fn running_on(data_dir: &Path, binary_name: &str) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            let first = cmdline.split_whitespace().next().unwrap_or("");
            let is_browser = first.ends_with(&format!("/{binary_name}"))
                || first == binary_name
                || (binary_name == "chromium" && first.ends_with("/chromium/chromium"));
            is_browser && !cmdline.contains("--type=") && {
                let dir = cmdline
                    .split_whitespace()
                    .find_map(|a| a.strip_prefix("--user-data-dir="))
                    .map(PathBuf::from);
                dir.as_deref() == Some(data_dir)
                    || (dir.is_none() && data_dir.ends_with(binary_name))
            }
        })
        .collect()
}

/// Close the browser using `data_dir` gracefully (SIGTERM; Chromium saves its
/// session on it) and wait for it and its profile lock to go.
pub fn close_browser(data_dir: &Path, binary_name: &str) -> anyhow::Result<bool> {
    let pids = running_on(data_dir, binary_name);
    if pids.is_empty() {
        return Ok(false);
    }
    for pid in &pids {
        unsafe { libc_kill(*pid as i32, 15) };
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if running_on(data_dir, binary_name).is_empty() && !lock_held(data_dir) {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    anyhow::bail!("{binary_name} did not close within 15s; profile data was not written")
}

/// After closing a browser to write its profile: mark the exit clean.
/// SIGTERM leaves `exit_type: Crashed`, and Chromium's next launch would then
/// bring back the whole previous session as well as the windows being opened.
pub fn mark_clean_exit(profile: &Path) -> anyhow::Result<()> {
    let path = profile.join("Preferences");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(());
    };
    let mut prefs: serde_json::Value = serde_json::from_slice(&bytes)?;
    if prefs["profile"]["exit_type"] == "Normal" {
        return Ok(());
    }
    prefs["profile"]["exit_type"] = serde_json::json!("Normal");
    prefs["profile"]["exited_cleanly"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_vec(&prefs)?)?;
    Ok(())
}

/// Chromium leaves `SingletonLock -> <host>-<pid>` behind on exit; the lock is
/// only held while that pid on this host is alive.
fn lock_held(data_dir: &Path) -> bool {
    let Ok(target) = std::fs::read_link(data_dir.join("SingletonLock")) else {
        return false;
    };
    let Some((host, pid)) = target.to_str().and_then(|t| t.rsplit_once('-')) else {
        return false;
    };
    let me = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    host == me.trim() && Path::new(&format!("/proc/{pid}")).exists()
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_scope_to_registrable_hosts() {
        let urls = vec![
            "https://api.github.com/repos".to_string(),
            "https://github.com/".to_string(),
            "https://www.bbc.co.uk/news".to_string(),
            "file:///home/u/todo.html".to_string(),
            "chrome://newtab/".to_string(),
        ];
        assert_eq!(sites_of(&urls), vec!["bbc.co.uk", "file", "github.com"]);
    }

    #[test]
    fn only_known_sensitive_items_can_be_preallowed() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".config/omaspace")).unwrap();
        std::fs::write(
            home.path().join(".config/omaspace/policy.json"),
            r#"{"allow_sensitive":["cookies","keyring"]}"#,
        )
        .unwrap();
        assert_eq!(policy_allowed(home.path()), vec!["cookies".to_string()]);
        assert!(
            policy_allowed(Path::new("/nonexistent")).is_empty(),
            "nothing sensitive by default"
        );
    }
}
