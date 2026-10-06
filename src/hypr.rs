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

fn hyprctl(args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("hyprctl")
        .args(args)
        .output()
        .context("running hyprctl")?;
    anyhow::ensure!(
        out.status.success(),
        "hyprctl {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?)
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
