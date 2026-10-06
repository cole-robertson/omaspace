//! Omarchy's own defaults, so omaspace knows where things live without
//! guessing: the browsers `omarchy-default-browser` / `omarchy-install-browser`
//! support, their profile dirs and launchers, the terminals
//! `omarchy-default-terminal` knows, and Omarchy web apps (`.desktop` files
//! that run `omarchy-launch-webapp <url>`).
//!
//! Extra browsers can be added in `~/.config/omaspace/browsers` (one per line:
//! `<class> <binary> <profile dir under ~/.config>`), for anything Omarchy
//! itself doesn't ship.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Browser {
    /// Hyprland window class.
    pub class: String,
    /// Executable to launch.
    pub binary: String,
    /// User-data dir under `~/.config` (Chromium family).
    pub config_dir: String,
}

/// The Chromium-family browsers Omarchy supports (`omarchy-default-browser`).
/// Firefox and Zen keep sessions in a compressed jsonlz4 store; omaspace
/// restores their windows as the app only, without tabs (see SPEC.md).
const CHROMIUM_FAMILY: &[(&str, &str, &str)] = &[
    ("chromium", "chromium", "chromium"),
    ("google-chrome", "google-chrome-stable", "google-chrome"),
    ("brave-browser", "brave", "BraveSoftware/Brave-Browser"),
    ("brave-origin", "brave-origin", "BraveSoftware/Brave-Origin"),
    ("microsoft-edge", "microsoft-edge-stable", "microsoft-edge"),
];

/// Terminals `omarchy-default-terminal` maps, plus Omarchy's own app-id for its
/// floating/presentation terminals.
pub const TERMINAL_CLASSES: &[&str] = &[
    "alacritty",
    "foot",
    "com.mitchellh.ghostty",
    "kitty",
    "org.omarchy.terminal",
];

/// Shells that can sit directly under a terminal.
pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "sh", "nu"];

/// Terminal editors Omarchy can set as default (`omarchy-default-editor`).
pub const TERMINAL_EDITORS: &[&str] = &["nvim", "vim", "helix", "hx"];

pub fn browsers(home: &Path) -> Vec<Browser> {
    let mut out: Vec<Browser> = CHROMIUM_FAMILY
        .iter()
        .map(|(class, binary, dir)| Browser {
            class: (*class).into(),
            binary: (*binary).into(),
            config_dir: (*dir).into(),
        })
        .collect();
    let extra = std::fs::read_to_string(home.join(".config/omaspace/browsers")).unwrap_or_default();
    out.extend(parse_extra_browsers(&extra));
    out
}

fn parse_extra_browsers(text: &str) -> Vec<Browser> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (class, binary, dir) = (f.next()?, f.next()?, f.next()?);
            (f.next().is_none() && !dir.contains("..") && !dir.starts_with('/')).then(|| Browser {
                class: class.into(),
                binary: binary.into(),
                config_dir: dir.into(),
            })
        })
        .collect()
}

pub fn browser_for_class<'a>(browsers: &'a [Browser], class: &str) -> Option<&'a Browser> {
    browsers
        .iter()
        .find(|b| b.class.eq_ignore_ascii_case(class))
}

/// The browser to reopen tabs in on this machine: the captured one if it is
/// installed here, else the user's Omarchy default browser, else Chromium.
pub fn launchable_browser<'a>(
    browsers: &'a [Browser],
    captured_class: &str,
) -> Option<&'a Browser> {
    let installed = |b: &&Browser| which(&b.binary).is_some();
    browser_for_class(browsers, captured_class)
        .filter(installed)
        .or_else(|| {
            default_browser_class()
                .and_then(|c| browser_for_class(browsers, &c))
                .filter(installed)
        })
        .or_else(|| browsers.iter().find(installed))
}

/// `xdg-settings get default-web-browser` → `brave-browser.desktop` → class.
fn default_browser_class() -> Option<String> {
    let out = std::process::Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(id.trim_end_matches(".desktop").to_string()).filter(|s| !s.is_empty())
}

/// The data dir of a running instance of `browser` (its `--user-data-dir`),
/// else that browser's default dir under `~/.config`.
pub fn running_data_dir(browser: &Browser, home: &Path) -> PathBuf {
    let default = home.join(".config").join(&browser.config_dir);
    // Several instances (e.g. a second --user-data-dir) can run at once. The
    // default profile wins whenever it exists, running or not; another
    // running profile is only used when there is no default one at all.
    let running: Vec<PathBuf> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            let target = which(&browser.binary).and_then(|p| std::fs::canonicalize(p).ok())?;
            let same_browser = exe.file_name() == target.file_name()
                || exe.parent().is_some_and(|d| {
                    d.ends_with(&browser.config_dir) || d.ends_with(&browser.binary)
                });
            if !same_browser {
                return None;
            }
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            if cmdline.contains("--type=") {
                return None; // a renderer/helper, not the browser process
            }
            Some(
                cmdline
                    .split_whitespace()
                    .find_map(|a| a.strip_prefix("--user-data-dir=").map(PathBuf::from))
                    .unwrap_or_else(|| default.clone()),
            )
        })
        .collect();
    if running.contains(&default) || default.exists() {
        default
    } else {
        running.into_iter().min().unwrap_or(default)
    }
}

/// A profile locked by a Chromium on another machine (a synced or copied home)
/// makes Chromium block on a modal dialog. Report it instead of launching.
pub fn foreign_lock(data_dir: &Path) -> Option<String> {
    let lock = std::fs::read_link(data_dir.join("SingletonLock")).ok()?;
    let (host, _) = lock.to_str()?.rsplit_once('-')?;
    let me = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
    (host != me.trim()).then(|| {
        format!(
            "{} is locked by a Chromium on {host}; close it there or remove {}/SingletonLock",
            data_dir.display(),
            data_dir.display()
        )
    })
}

/// Desktop entry directories in Omarchy's search order (see
/// `omarchy-launch-browser`).
pub fn application_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/share/applications"),
        home.join(".nix-profile/share/applications"),
        PathBuf::from("/usr/share/applications"),
    ]
}

/// An Omarchy web app's URL, if `desktop_id` is one (`Exec=omarchy-launch-webapp <url>`).
pub fn webapp_url(desktop_id: &str, home: &Path) -> Option<String> {
    application_dirs(home).iter().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join(desktop_id)).ok()?;
        text.lines()
            .find_map(|l| l.strip_prefix("Exec="))?
            .strip_prefix("omarchy-launch-webapp ")
            .map(|url| url.split_whitespace().next().unwrap_or("").to_string())
    })
}

/// Omarchy web apps run as Chromium `--app=<url>` windows whose class is
/// `chrome-<host><path with / as _>__-<profile>`. Return the installed web app
/// `.desktop` whose URL produces that class.
pub fn webapp_for_class(class: &str, home: &Path) -> Option<(String, String)> {
    let key = class
        .strip_prefix("chrome-")?
        .split("__-")
        .next()?
        .to_string();
    application_dirs(home).iter().find_map(|dir| {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let url = webapp_url(&id, home)?;
            let host_path = url
                .split("://")
                .nth(1)?
                .trim_end_matches('/')
                .replace('/', "_");
            (host_path == key || host_path.split('_').next() == Some(key.as_str()))
                .then_some((id, url))
        })
    })
}

pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        return Path::new(bin).is_file().then(|| PathBuf::from(bin));
    }
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(bin))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chromium_browser_omarchy_supports_is_known() {
        let browsers = browsers(Path::new("/nonexistent"));
        for class in [
            "chromium",
            "google-chrome",
            "brave-browser",
            "brave-origin",
            "microsoft-edge",
        ] {
            assert!(browser_for_class(&browsers, class).is_some(), "{class}");
        }
    }

    #[test]
    fn web_app_windows_map_back_to_their_desktop_file() {
        let home = tempfile::tempdir().unwrap();
        let apps = home.path().join(".local/share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("Basecamp.desktop"),
            "[Desktop Entry]\nExec=omarchy-launch-webapp https://launchpad.37signals.com\n",
        )
        .unwrap();
        assert_eq!(
            webapp_for_class("chrome-launchpad.37signals.com__-Default", home.path()),
            Some((
                "Basecamp.desktop".into(),
                "https://launchpad.37signals.com".into()
            ))
        );
        assert_eq!(
            webapp_for_class("chrome-app.hey.com__-Default", home.path()),
            None
        );
        assert_eq!(webapp_for_class("chromium", home.path()), None);
    }

    #[test]
    fn extra_browsers_extend_the_defaults_but_stay_under_config() {
        let extra = parse_extra_browsers(
            "vivaldi-stable vivaldi vivaldi  # mine\nevil x ../../.ssh\nabs x /etc\n",
        );
        assert_eq!(
            extra,
            vec![Browser {
                class: "vivaldi-stable".into(),
                binary: "vivaldi".into(),
                config_dir: "vivaldi".into()
            }]
        );
    }
}
