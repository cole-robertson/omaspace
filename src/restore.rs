//! Reopen a snapshot's windows on this desktop. Each window is launched with
//! Hyprland's per-launch rules (`hl.dsp.exec_cmd(cmd, { workspace = "N silent" })`)
//! so it lands on its original workspace without taking focus.
//!
//! Only fixed launchers run: Chromium with URLs, the Omarchy terminal in a
//! directory (optionally attaching tmux or opening nvim), or an installed
//! `.desktop` file. Nothing from the snapshot is ever run as a shell string;
//! every argument is quoted.

use crate::hypr;
use crate::omarchy::{self, which};
use crate::snapshot::{self, Content, Snapshot, Window};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Restored {
        workspace: i64,
        title: String,
    },
    Skipped {
        workspace: i64,
        title: String,
        reason: String,
    },
    Failed {
        workspace: i64,
        title: String,
        error: String,
    },
}

#[derive(Debug, Serialize, Default)]
pub struct Report {
    /// One line per browser whose profile data was applied.
    pub profile: Vec<String>,
    pub restored: usize,
    pub skipped: usize,
    pub failed: usize,
    pub windows: Vec<Outcome>,
}

pub fn restore(snapshot: &Snapshot, home: &Path) -> anyhow::Result<Report> {
    snapshot.validate()?;
    let mut report = Report::default();
    // Profile data first: like cua's receiver, the browser for that profile is
    // closed while its files are written, then the windows below reopen it.
    for (class, data) in &snapshot.browser_data {
        let browsers = omarchy::browsers(home);
        let Some(target) = omarchy::launchable_browser(&browsers, class) else {
            report
                .profile
                .push(format!("{class}: no browser to receive it"));
            continue;
        };
        let data_dir = omarchy::running_data_dir(target, home);
        let binary = target
            .binary
            .rsplit('/')
            .next()
            .unwrap_or(&target.binary)
            .to_string();
        let closed = crate::profile::close_browser(&data_dir, &binary)?;
        let applied = crate::profile::apply(&data_dir.join("Default"), data)?;
        if closed {
            crate::profile::mark_clean_exit(&data_dir.join("Default"))?;
        }
        report.profile.push(format!(
            "{}: {} site(s), {} storage item(s), {} cookie(s){}{}",
            target.class,
            data.sites.len(),
            applied.local_storage,
            applied.cookies,
            if data.allowed_sensitive.is_empty() {
                " (sign-ins withheld)".to_string()
            } else {
                String::new()
            },
            if closed { ", browser restarted" } else { "" }
        ));
    }
    for workspace in &snapshot.workspaces {
        for window in &workspace.windows {
            let outcome = match launch_command(window, home) {
                Err(reason) => Outcome::Skipped {
                    workspace: workspace.id,
                    title: window.title.clone(),
                    reason,
                },
                Ok(argv) => match launch_on(&argv, workspace.id) {
                    Ok(()) => Outcome::Restored {
                        workspace: workspace.id,
                        title: window.title.clone(),
                    },
                    Err(e) => Outcome::Failed {
                        workspace: workspace.id,
                        title: window.title.clone(),
                        error: e.to_string(),
                    },
                },
            };
            match outcome {
                Outcome::Restored { .. } => report.restored += 1,
                Outcome::Skipped { .. } => report.skipped += 1,
                Outcome::Failed { .. } => report.failed += 1,
            }
            report.windows.push(outcome);
        }
    }
    Ok(report)
}

/// Launch `argv` onto `workspace` without taking focus, then confirm a new
/// window appeared and is on that workspace. Hyprland's launch rule only binds
/// to the process it starts; a second Chromium window is opened by the already
/// running browser, so a window that lands elsewhere is moved explicitly.
fn launch_on(argv: &[String], workspace: i64) -> anyhow::Result<()> {
    let before: std::collections::HashSet<String> =
        hypr::clients()?.into_iter().map(|c| c.address).collect();
    let target = hypr::lua_string(&workspace.to_string());
    let rules = format!(
        "{{ workspace = {} }}",
        hypr::lua_string(&format!("{workspace} silent"))
    );
    hypr::dispatch(&format!(
        "hl.dsp.exec_cmd({}, {rules})",
        hypr::lua_string(&shell_join(argv))
    ))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let new: Vec<_> = hypr::clients()?
            .into_iter()
            .filter(|c| !before.contains(&c.address))
            .collect();
        if let Some(window) = new.first() {
            if window.workspace.id != workspace {
                let address = hypr::lua_string(&format!("address:{}", window.address));
                hypr::dispatch(&format!(
                    "hl.dsp.window.move({{ workspace = {target}, follow = false, window = {address} }})"
                ))?;
            }
            return Ok(());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "no window appeared within 10s"
        );
    }
}

/// The argv that reopens `window` on this machine, or why it can't be.
pub fn launch_command(window: &Window, home: &Path) -> Result<Vec<String>, String> {
    let dir = |portable: &str| {
        let path = snapshot::from_portable(portable, home);
        if path.is_dir() {
            path
        } else {
            home.to_path_buf()
        }
    };
    let uwsm = |mut argv: Vec<String>| {
        let mut full = vec!["uwsm-app".to_string(), "--".to_string()];
        full.append(&mut argv);
        full
    };
    match &window.content {
        Content::Browser { browser, urls, .. } => {
            // The captured browser if it is installed here, else this
            // machine's Omarchy default browser.
            let browsers = omarchy::browsers(home);
            let target = omarchy::launchable_browser(&browsers, browser)
                .ok_or("no Chromium-family browser is installed")?;
            // Open in the profile that's already running, so the tabs join the
            // user's signed-in browser instead of starting a second instance.
            let data_dir = omarchy::running_data_dir(target, home);
            if let Some(reason) = omarchy::foreign_lock(&data_dir) {
                return Err(reason);
            }
            // --no-first-run: a restarted or freshly received profile must not
            // stop on Chromium's first-run terms page.
            let mut argv = vec![
                target.binary.clone(),
                format!("--user-data-dir={}", data_dir.display()),
                "--no-first-run".to_string(),
                "--new-window".to_string(),
            ];
            argv.extend(urls.iter().cloned());
            Ok(uwsm(argv))
        }
        Content::WebApp { url, .. } => {
            which("omarchy-launch-webapp").ok_or("omarchy-launch-webapp is not available")?;
            Ok(vec!["omarchy-launch-webapp".into(), url.clone()])
        }
        Content::Terminal { cwd, tmux_session } => {
            let mut argv = vec![
                "xdg-terminal-exec".to_string(),
                format!("--dir={}", dir(cwd).display()),
            ];
            if let Some(name) = tmux_session {
                which("tmux").ok_or("tmux is not installed")?;
                argv.extend(["-e", "tmux", "new-session", "-A", "-s", name].map(String::from));
            }
            Ok(uwsm(argv))
        }
        Content::Editor { editor, cwd, files } => {
            if !omarchy::TERMINAL_EDITORS.contains(&editor.as_str()) {
                return Err(format!("{editor} is not a known terminal editor"));
            }
            which(editor).ok_or_else(|| format!("{editor} is not installed"))?;
            let mut argv = vec![
                "xdg-terminal-exec".to_string(),
                format!("--dir={}", dir(cwd).display()),
                "-e".to_string(),
                editor.clone(),
            ];
            for file in files {
                let path = snapshot::from_portable(file, home);
                if path.exists() {
                    argv.push(path.display().to_string());
                }
            }
            Ok(uwsm(argv))
        }
        Content::App { desktop_id } => {
            if !omarchy::application_dirs(home)
                .iter()
                .any(|d| d.join(desktop_id).is_file())
            {
                return Err(format!("{desktop_id} is not installed here"));
            }
            Ok(vec!["uwsm-app".into(), "--".into(), desktop_id.clone()])
        }
    }
}

/// Join argv for Hyprland's exec, single-quoting every argument.
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| format!("'{}'", a.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(content: Content) -> Window {
        Window {
            class: "x".into(),
            title: "x".into(),
            floating: false,
            at: [0, 0],
            size: [1, 1],
            fullscreen: 0,
            content,
        }
    }

    #[test]
    fn every_argument_is_quoted_so_nothing_runs_as_shell() {
        let joined = shell_join(&["echo".into(), "a'; rm -rf ~; '".into(), "$(id)".into()]);
        assert_eq!(joined, r#"'echo' 'a'\''; rm -rf ~; '\''' '$(id)'"#);
    }

    #[test]
    fn terminal_reopens_in_its_directory_or_falls_back_home() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("src/x")).unwrap();
        let argv = launch_command(
            &window(Content::Terminal {
                cwd: "~/src/x".into(),
                tmux_session: None,
            }),
            home.path(),
        )
        .unwrap();
        assert_eq!(argv[2], "xdg-terminal-exec");
        assert_eq!(
            argv[3],
            format!("--dir={}", home.path().join("src/x").display())
        );
        let missing = launch_command(
            &window(Content::Terminal {
                cwd: "~/gone".into(),
                tmux_session: None,
            }),
            home.path(),
        )
        .unwrap();
        assert_eq!(missing[3], format!("--dir={}", home.path().display()));
    }

    #[test]
    fn apps_must_be_installed_on_this_machine() {
        let home = tempfile::tempdir().unwrap();
        let err = launch_command(
            &window(Content::App {
                desktop_id: "nope-xyz.desktop".into(),
            }),
            home.path(),
        );
        assert!(err.unwrap_err().contains("not installed"));
    }
}
