//! Hyprland: read clients and workspaces, run Lua dispatches (Omarchy's
//! Hyprland uses the Lua config, so `hyprctl dispatch` takes `hl.dsp.*`).

use anyhow::Context;
use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct Client {
    pub address: String,
    pub class: String,
    #[serde(rename = "initialClass", default)]
    pub initial_class: String,
    pub title: String,
    pub pid: i64,
    pub workspace: WorkspaceRef,
    pub floating: bool,
    pub at: [i64; 2],
    pub size: [i64; 2],
    #[serde(default)]
    pub fullscreen: i64,
    #[serde(default = "mapped_default")]
    pub mapped: bool,
    #[serde(default)]
    pub hidden: bool,
    /// 0 = focused most recently; larger is further back in the stack.
    #[serde(rename = "focusHistoryID", default)]
    pub focus_history: i64,
}

fn mapped_default() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

/// How long one hyprctl call may take. Hyprland answers in milliseconds; a
/// call that takes longer means the compositor is busy or stuck, and waiting
/// on it would stall the caller (and, through it, the live view) as well.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// After a call times out, further calls fail fast for this long instead of
/// queueing more work on a compositor that isn't answering.
const BACKOFF: std::time::Duration = std::time::Duration::from_secs(10);

static STUCK_UNTIL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn hyprctl(args: &[&str]) -> anyhow::Result<String> {
    ctl(args, CALL_TIMEOUT)
}

/// Run `hyprctl args` and return its stdout, killing it after `timeout`.
pub fn ctl(args: &[&str], timeout: std::time::Duration) -> anyhow::Result<String> {
    ctl_with("hyprctl", args, timeout)
}

fn ctl_with(program: &str, args: &[&str], timeout: std::time::Duration) -> anyhow::Result<String> {
    if let Some(until) = *STUCK_UNTIL.lock().unwrap() {
        anyhow::ensure!(
            std::time::Instant::now() >= until,
            "Hyprland isn't answering; not sending hyprctl {args:?}"
        );
    }
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("running hyprctl")?;
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let read_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut s);
        s
    });
    let read_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut s);
        s
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            *STUCK_UNTIL.lock().unwrap() = Some(std::time::Instant::now() + BACKOFF);
            anyhow::bail!("hyprctl {args:?} didn't answer within {timeout:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    *STUCK_UNTIL.lock().unwrap() = None;
    let (stdout, stderr) = (
        read_out.join().unwrap_or_default(),
        read_err.join().unwrap_or_default(),
    );
    anyhow::ensure!(
        status.success(),
        "hyprctl {args:?}: {}",
        String::from_utf8_lossy(&stderr)
    );
    Ok(String::from_utf8(stdout)?)
}

/// `hyprctl eval <lua>` (registering rules, which `dispatch` can't).
pub fn eval(lua: &str) -> anyhow::Result<()> {
    hyprctl(&["eval", lua]).map(|_| ())
}

/// Every monitor, including disabled and headless ones.
pub fn monitors_all() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_str(&hyprctl(&["-j", "monitors", "all"])?)?)
}

/// The focused window's address, if any.
pub fn active_window() -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&hyprctl(&["-j", "activewindow"]).ok()?).ok()?;
    v["address"].as_str().map(String::from)
}

/// Whether a Hyprland session is reachable from this process.
pub fn available() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() && hyprctl(&["-j", "version"]).is_ok()
}

pub fn clients() -> anyhow::Result<Vec<Client>> {
    Ok(serde_json::from_str(&hyprctl(&["-j", "clients"])?)?)
}

pub fn active_workspace() -> anyhow::Result<i64> {
    #[derive(Deserialize)]
    struct Active {
        id: i64,
    }
    Ok(serde_json::from_str::<Active>(&hyprctl(&["-j", "activeworkspace"])?)?.id)
}

/// Pixel size of the first real (non-virtual) monitor.
pub fn primary_monitor_size() -> anyhow::Result<(u32, u32)> {
    let list: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "monitors"])?)?;
    let mon = list
        .as_array()
        .and_then(|l| {
            l.iter()
                .find(|m| !m["name"].as_str().unwrap_or("").starts_with("OSP-"))
        })
        .context("no monitor")?;
    Ok((
        mon["width"].as_u64().unwrap_or(1920) as u32,
        mon["height"].as_u64().unwrap_or(1080) as u32,
    ))
}

/// Workspaces 1-9 (and any others in use), the active one, and each window's
/// class and title: what the Spaces panel draws for a machine.
pub fn workspaces_summary() -> anyhow::Result<serde_json::Value> {
    let clients = clients()?;
    let active = active_workspace()?;
    let mut ids: Vec<i64> = (1..=9).collect();
    for c in &clients {
        if c.workspace.id > 0 && !ids.contains(&c.workspace.id) {
            ids.push(c.workspace.id);
        }
    }
    ids.sort();
    let workspaces: Vec<_> = ids
        .iter()
        .map(|id| {
            let windows: Vec<_> = clients
                .iter()
                .filter(|c| c.mapped && c.workspace.id == *id)
                .map(|c| serde_json::json!({"address": c.address, "class": c.class, "title": c.title}))
                .collect();
            serde_json::json!({"id": id, "windows": windows})
        })
        .collect();
    Ok(serde_json::json!({"active": active, "workspaces": workspaces}))
}

/// Run a Lua dispatcher expression, e.g. `hl.dsp.focus({ workspace = "3" })`.
pub fn dispatch(lua: &str) -> anyhow::Result<()> {
    let out = hyprctl(&["dispatch", lua])?;
    anyhow::ensure!(!out.contains("error"), "hyprctl dispatch {lua}: {out}");
    Ok(())
}

pub fn lua_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    /// A hyprctl that never answers (Hyprland stuck): the call gives up on
    /// time, and the next calls fail fast instead of piling up behind it.
    #[test]
    fn a_stuck_hyprland_never_stalls_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("hyprctl");
        std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let fake = fake.to_str().unwrap();
        let t = std::time::Instant::now();
        let first = super::ctl_with(
            fake,
            &["-j", "version"],
            std::time::Duration::from_millis(300),
        );
        assert!(first.is_err(), "a hung hyprctl is an error, not a wait");
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "gave up on time: {:?}",
            t.elapsed()
        );
        let t = std::time::Instant::now();
        let second = super::ctl_with(fake, &["-j", "version"], std::time::Duration::from_secs(5));
        assert!(second.unwrap_err().to_string().contains("isn't answering"));
        assert!(
            t.elapsed() < std::time::Duration::from_millis(100),
            "later calls fail fast"
        );
        *super::STUCK_UNTIL.lock().unwrap() = None;
    }
}
