//! The snapshot format (`omaspace.snapshot.v1`) and home-relative paths.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "omaspace.snapshot.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub format: String,
    pub id: String,
    pub source: String,
    pub taken_at: u64,
    pub workspaces: Vec<Workspace>,
    /// Per browser class: profile data for the sites open in this snapshot
    /// (follows cua Spaces' defaults; see SPEC.md).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub browser_data: std::collections::BTreeMap<String, crate::profile::ProfileData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub class: String,
    pub title: String,
    pub floating: bool,
    pub at: [i64; 2],
    pub size: [i64; 2],
    pub fullscreen: i64,
    #[serde(flatten)]
    pub content: Content,
}

/// What a window was showing; this is all a restore can relaunch. There is no
/// free-form command line: each kind maps to a fixed launcher on restore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    /// `browser` is the Hyprland class of the browser it was open in.
    Browser {
        browser: String,
        urls: Vec<String>,
        active: usize,
    },
    Terminal {
        cwd: String,
        tmux_session: Option<String>,
    },
    Editor {
        editor: String,
        cwd: String,
        files: Vec<String>,
    },
    App {
        desktop_id: String,
    },
    /// An Omarchy web app (`omarchy-launch-webapp <url>`).
    WebApp {
        desktop_id: String,
        url: String,
    },
}

/// `/home/u/src/x` → `~/src/x` when under `home`, so it rebases onto another
/// machine's home. Paths elsewhere stay absolute.
pub fn to_portable(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The inverse of [`to_portable`] on the destination machine.
pub fn from_portable(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

impl Snapshot {
    /// Put every window on workspace `id` (dropping one window onto a
    /// chosen workspace of the other machine).
    pub fn onto_workspace(&mut self, id: i64) {
        let windows = self.workspaces.drain(..).flat_map(|w| w.windows).collect();
        self.workspaces = vec![Workspace {
            id,
            name: id.to_string(),
            windows,
        }];
    }

    /// Every URL open in the snapshot's browsers and web apps.
    pub fn urls(&self) -> Vec<String> {
        let mut out = Vec::new();
        for window in self.workspaces.iter().flat_map(|w| &w.windows) {
            match &window.content {
                Content::Browser { urls, .. } => out.extend(urls.iter().cloned()),
                Content::WebApp { url, .. } => out.push(url.clone()),
                _ => {}
            }
        }
        out
    }

    pub fn window_count(&self) -> usize {
        self.workspaces.iter().map(|w| w.windows.len()).sum()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.format == FORMAT,
            "unsupported snapshot format {:?}",
            self.format
        );
        // Sign-ins may only be for sites open in this snapshot's windows: a
        // sender can't plant cookies for every site ("*" is whole-profile
        // data, for a machine's own agents only, never sent between peers).
        let open = crate::profile::sites_of(&self.urls());
        for (class, data) in &self.browser_data {
            for site in &data.sites {
                anyhow::ensure!(
                    open.contains(site),
                    "{class}: sign-ins for {site:?}, which isn't open in this snapshot"
                );
            }
        }
        for window in self.workspaces.iter().flat_map(|w| &w.windows) {
            match &window.content {
                Content::Browser { browser, urls, .. } => {
                    anyhow::ensure!(
                        !browser.is_empty()
                            && browser
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                        "invalid browser {browser:?}"
                    );
                    for url in urls {
                        anyhow::ensure!(
                            url.starts_with("http://")
                                || url.starts_with("https://")
                                || url.starts_with("chrome://newtab")
                                || url.starts_with("file://"),
                            "refusing browser URL scheme in {url:?}"
                        );
                    }
                }
                Content::WebApp { desktop_id, url } => anyhow::ensure!(
                    desktop_id.ends_with(".desktop")
                        && !desktop_id.contains('/')
                        && url.starts_with("https://"),
                    "invalid web app {desktop_id:?} {url:?}"
                ),
                Content::App { desktop_id } => anyhow::ensure!(
                    desktop_id.ends_with(".desktop") && !desktop_id.contains('/'),
                    "invalid desktop id {desktop_id:?}"
                ),
                Content::Terminal {
                    tmux_session: Some(name),
                    ..
                } => anyhow::ensure!(
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                    "invalid tmux session name {name:?}"
                ),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_paths_travel_relative_and_rebase() {
        let home = Path::new("/home/me");
        assert_eq!(to_portable(Path::new("/home/me/src/x"), home), "~/src/x");
        assert_eq!(to_portable(Path::new("/home/me"), home), "~");
        assert_eq!(to_portable(Path::new("/etc"), home), "/etc");
        let other = Path::new("/home/ana");
        assert_eq!(
            from_portable("~/src/x", other),
            Path::new("/home/ana/src/x")
        );
        assert_eq!(from_portable("~", other), other);
        assert_eq!(from_portable("/etc", other), Path::new("/etc"));
    }

    fn snapshot_with(content: Content) -> Snapshot {
        Snapshot {
            format: FORMAT.into(),
            id: "t".into(),
            source: "a".into(),
            taken_at: 0,
            browser_data: Default::default(),
            workspaces: vec![Workspace {
                id: 1,
                name: "1".into(),
                windows: vec![Window {
                    class: "x".into(),
                    title: "x".into(),
                    floating: false,
                    at: [0, 0],
                    size: [1, 1],
                    fullscreen: 0,
                    content,
                }],
            }],
        }
    }

    #[test]
    fn restore_refuses_dangerous_payloads() {
        let js = Content::Browser {
            browser: "chromium".into(),
            urls: vec!["javascript:alert(1)".into()],
            active: 0,
        };
        assert!(snapshot_with(js).validate().is_err());
        let app = Content::App {
            desktop_id: "../../bin/sh".into(),
        };
        assert!(snapshot_with(app).validate().is_err());
        let tmux = Content::Terminal {
            cwd: "~".into(),
            tmux_session: Some("a;rm -rf ~".into()),
        };
        assert!(snapshot_with(tmux).validate().is_err());
        let ok = Content::Browser {
            browser: "chromium".into(),
            urls: vec!["https://ok.example/".into()],
            active: 0,
        };
        assert!(snapshot_with(ok).validate().is_ok());
    }

    #[test]
    fn sign_ins_only_for_sites_that_are_open() {
        let open = Content::Browser {
            browser: "chromium".into(),
            urls: vec!["https://github.com/x".into()],
            active: 0,
        };
        let with_sites = |sites: &[&str]| {
            let mut s = snapshot_with(open.clone());
            s.browser_data.insert(
                "chromium".into(),
                crate::profile::ProfileData {
                    sites: sites.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
            );
            s
        };
        assert!(with_sites(&["github.com"]).validate().is_ok());
        assert!(
            with_sites(&["*"]).validate().is_err(),
            "whole-profile data never travels between machines"
        );
        assert!(
            with_sites(&["github.com", "bank.example"])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn json_round_trip_keeps_the_kind_tag() {
        let s = snapshot_with(Content::Terminal {
            cwd: "~/src".into(),
            tmux_session: Some("work".into()),
        });
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"terminal\""));
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), s);
    }
}
