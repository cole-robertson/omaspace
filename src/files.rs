//! Optional project-folder transfer (`--with-files`): the folders a
//! snapshot's terminals and editors are open in travel with it, so work done on
//! one machine comes back with the windows. rsync over ssh, tailnet hostnames
//! only, newer files on the receiving side are kept (`--update`).
//!
//! Only folders strictly inside `$HOME` travel: never `$HOME` itself, never a
//! dot-folder (`~/.ssh`, `~/.config`, ...), never anything outside home.

use crate::snapshot::{Content, Snapshot};
use std::collections::BTreeSet;
use std::process::Command;

/// The portable (`~/…`) folders of `snapshot` that may travel.
pub fn folders(snapshot: &Snapshot) -> Vec<String> {
    let mut out = BTreeSet::new();
    for window in snapshot.workspaces.iter().flat_map(|w| &w.windows) {
        let cwd = match &window.content {
            Content::Terminal { cwd, .. } | Content::Editor { cwd, .. } => cwd,
            _ => continue,
        };
        if travels(cwd) {
            out.insert(cwd.clone());
        }
    }
    // Drop folders nested inside another selected folder.
    let all: Vec<String> = out.iter().cloned().collect();
    out.into_iter()
        .filter(|f| {
            !all.iter()
                .any(|p| p != f && f.starts_with(&format!("{p}/")))
        })
        .collect()
}

fn travels(portable: &str) -> bool {
    let Some(rest) = portable.strip_prefix("~/") else {
        return false;
    };
    !rest.is_empty()
        && rest
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && part != "..")
}

/// Copy `folders` from `from` to `to` (each `"local"` or a tailnet hostname).
/// Returns one line per folder; any rsync failure is an error, never skipped.
pub fn sync(folders: &[String], from: &str, to: &str) -> anyhow::Result<Vec<String>> {
    let mut report = Vec::new();
    for folder in folders {
        anyhow::ensure!(travels(folder), "refusing to transfer {folder}");
        let rel = folder.trim_start_matches("~/");
        let side = |host: &str| {
            if host == "local" {
                format!("{}/{rel}/", home())
            } else {
                format!("{host}:{rel}/")
            }
        };
        if to != "local" {
            let ok = Command::new("ssh")
                .args(["-o", "BatchMode=yes", to, "mkdir", "-p", "--", rel])
                .status()?;
            anyhow::ensure!(ok.success(), "could not create {folder} on {to}");
        } else {
            std::fs::create_dir_all(format!("{}/{rel}", home()))?;
        }
        let out = Command::new("rsync")
            .args([
                "-a",
                "--update",
                "--itemize-changes",
                "-e",
                "ssh -o BatchMode=yes",
                &side(from),
                &side(to),
            ])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "rsync {folder} {from} -> {to}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        let changed = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.starts_with('>') || l.starts_with('<'))
            .count();
        report.push(format!("{folder}: {changed} file(s) updated"));
    }
    Ok(report)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{FORMAT, Window, Workspace};

    fn snapshot(dirs: &[&str]) -> Snapshot {
        Snapshot {
            format: FORMAT.into(),
            id: "t".into(),
            source: "a".into(),
            taken_at: 0,
            browser_data: Default::default(),
            workspaces: vec![Workspace {
                id: 1,
                name: "1".into(),
                windows: dirs
                    .iter()
                    .map(|d| Window {
                        class: "foot".into(),
                        title: "t".into(),
                        floating: false,
                        at: [0, 0],
                        size: [1, 1],
                        fullscreen: 0,
                        content: Content::Terminal {
                            cwd: (*d).into(),
                            tmux_session: None,
                        },
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn only_project_folders_inside_home_travel() {
        let s = snapshot(&[
            "~",
            "~/.ssh",
            "~/.config/hypr",
            "/etc",
            "~/src/app",
            "~/src/app/web",
            "~/notes",
            "~/src/../.ssh",
        ]);
        assert_eq!(
            folders(&s),
            vec!["~/notes".to_string(), "~/src/app".to_string()]
        );
    }
}
