//! Desktop apps for agents, on the agent's own workspace.
//!
//! Terminals: an Omarchy terminal attached to a tmux session the agent owns
//! (`tmux -L omaspace-agents`). The agent types with `send-keys` and reads the
//! screen with `capture-pane`: no input events reach your session at all.
//!
//! Every other app (GTK, Qt, Omarchy plugins, …): through cua-driver (the
//! omaspace-driver package). Its accessibility route clicks named buttons and
//! sets fields of a window that isn't focused; for allowlisted apps it can
//! also type through cua's Hyprland plugin, on an input seat of its own. Every
//! call here is checked first: the window must be on the agent's workspace, so
//! an agent can never act on your windows.

use crate::{agent, hypr};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

/// The tmux server agent terminals run on (separate from any you use).
const TMUX: &str = "omaspace-agents";
/// Window class of agent terminals (kept from taking focus, like browsers).
pub const TERM_CLASS: &str = "omaspace-agent-term";

fn tmux(args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("tmux")
        .arg("-L")
        .arg(TMUX)
        .args(args)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "tmux {}: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A session name tmux accepts, unique to the agent.
fn session(agent: &str, name: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    };
    format!("{}--{}", clean(agent), clean(name))
}

/// Open a terminal on the agent's workspace, in `dir` (default: home).
pub fn open_terminal(
    home: &Path,
    agent: &str,
    name: &str,
    dir: Option<&str>,
) -> anyhow::Result<Value> {
    let space = agent::space_of(home, agent)?;
    let s = session(agent, name);
    let cwd = dir
        .map(|d| crate::snapshot::from_portable(d, home))
        .unwrap_or_else(|| home.to_path_buf());
    anyhow::ensure!(cwd.is_dir(), "{} is not a folder", cwd.display());
    if tmux(&["has-session", "-t", &s]).is_err() {
        tmux(&[
            "new-session",
            "-d",
            "-s",
            &s,
            "-x",
            "160",
            "-y",
            "45",
            "-c",
            &cwd.to_string_lossy(),
        ])?;
    }
    agent::ensure_window_rule_for(TERM_CLASS);
    let argv = vec![
        terminal_binary(),
        format!("--app-id={TERM_CLASS}"),
        "tmux".into(),
        "-L".into(),
        TMUX.into(),
        "attach".into(),
        "-t".into(),
        s.clone(),
    ];
    hypr::dispatch(&format!(
        "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
        hypr::lua_string(&crate::restore::shell_join(&argv)),
        hypr::lua_string(&format!("{} silent", space.workspace))
    ))?;
    Ok(json!({"terminal": name, "workspace": space.workspace, "dir": cwd}))
}

/// foot is Omarchy's terminal and takes `--app-id`; fall back to it by name.
fn terminal_binary() -> String {
    crate::omarchy::which("foot")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "foot".into())
}

/// Type literal text (no Enter unless the text ends with a newline).
pub fn terminal_type(agent: &str, name: &str, text: &str) -> anyhow::Result<()> {
    let s = session(agent, name);
    let (body, enter) = match text.strip_suffix('\n') {
        Some(b) => (b, true),
        None => (text, false),
    };
    if !body.is_empty() {
        tmux(&["send-keys", "-t", &s, "-l", body])?;
    }
    if enter {
        tmux(&["send-keys", "-t", &s, "Enter"])?;
    }
    Ok(())
}

/// Press keys by tmux name: Enter, Escape, Tab, Up, C-c, C-d, …
pub fn terminal_key(agent: &str, name: &str, keys: &[String]) -> anyhow::Result<()> {
    let s = session(agent, name);
    let mut args = vec!["send-keys", "-t", s.as_str()];
    args.extend(keys.iter().map(String::as_str));
    tmux(&args)?;
    Ok(())
}

/// The visible screen (plus `history` lines of scrollback).
pub fn terminal_read(agent: &str, name: &str, history: u32) -> anyhow::Result<String> {
    let s = session(agent, name);
    let start = format!("-{history}");
    let out = tmux(&["capture-pane", "-p", "-J", "-t", &s, "-S", &start])?;
    Ok(out.trim_end().to_string())
}

/// Run a command and wait (up to `timeout`) for it to finish; returns what it
/// printed. Done by a marker the shell echoes after the command.
pub fn terminal_run(
    agent: &str,
    name: &str,
    command: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<Value> {
    let marker = format!(
        "__omaspace_done_{}",
        std::process::id()
            ^ (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .subsec_nanos())
    );
    let before = terminal_read(agent, name, 2000)?.lines().count();
    terminal_type(agent, name, &format!("{command}; echo {marker} $?\n"))?;
    let end = std::time::Instant::now() + timeout;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let screen = terminal_read(agent, name, 2000)?;
        // The marker's line from echo (not the typed command line, which also
        // contains it, followed by " $?").
        if let Some(line) = screen.lines().rev().find(|l| l.starts_with(&marker)) {
            let code: i32 = line
                .trim_start_matches(&marker)
                .trim()
                .parse()
                .unwrap_or(-1);
            let lines: Vec<&str> = screen.lines().collect();
            let out_end = lines
                .iter()
                .rposition(|l| l.starts_with(&marker))
                .unwrap_or(lines.len());
            let start = before.saturating_sub(1).min(out_end);
            let output = lines[start..out_end]
                .iter()
                .filter(|l| !l.contains(&format!("echo {marker}")))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(json!({"exit_code": code, "output": output.trim()}));
        }
        anyhow::ensure!(
            std::time::Instant::now() < end,
            "still running after {}s (use terminal_read to check on it)",
            timeout.as_secs()
        );
    }
}

pub fn close_terminals(agent: &str) {
    let prefix = format!("{}--", session(agent, "").trim_end_matches("--"));
    if let Ok(list) = tmux(&["list-sessions", "-F", "#{session_name}"]) {
        for s in list.lines().filter(|s| s.starts_with(&prefix)) {
            let _ = tmux(&["kill-session", "-t", s]);
        }
    }
}

// ---- any app, through cua-driver ----------------------------------------------

/// The driver session for an agent: a short public label, the same on every
/// call, so a token from app_read is still good for the click after it.
fn sess(agent: &str) -> String {
    format!("omaspace-{}", session(agent, "apps"))
}

/// cua-driver from the omaspace-driver package (or one on PATH).
fn driver() -> anyhow::Result<std::path::PathBuf> {
    let packaged = Path::new("/usr/lib/omaspace-driver/cua-driver");
    if packaged.is_file() {
        return Ok(packaged.into());
    }
    crate::omarchy::which("cua-driver").ok_or_else(|| {
        anyhow::anyhow!("cua-driver isn't installed (install the omaspace-driver package)")
    })
}

/// One cua-driver tool call; its JSON result. `session` keeps an agent's
/// snapshots (element tokens from app_read) valid for its later clicks.
fn driver_call(tool: &str, args: &Value) -> anyhow::Result<Value> {
    driver_call_in(None, tool, args)
}

fn driver_call_in(session: Option<&str>, tool: &str, args: &Value) -> anyhow::Result<Value> {
    match driver_call_once(session, tool, args) {
        // The driver ended this session (an agent released its space, then
        // came back under the same name): start it again and retry once.
        Err(e) if session.is_some() && e.to_string().contains("session has ended") => {
            driver_call_once(None, "start_session", &json!({"session": session}))?;
            driver_call_once(session, tool, args)
        }
        r => r,
    }
}

fn driver_call_once(session: Option<&str>, tool: &str, args: &Value) -> anyhow::Result<Value> {
    let mut args = args.clone();
    if let Some(s) = session {
        args["session"] = json!(s);
    }
    let out = Command::new(driver()?)
        .args(["call", tool, &args.to_string()])
        .env("CUA_DRIVER_RS_ENABLE_WAYLAND", "1")
        .env("CUA_TELEMETRY", "0")
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(text.trim()).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::anyhow!(
            "cua-driver {tool}: {}",
            if err.trim().is_empty() {
                text.trim().to_string()
            } else {
                err.trim().to_string()
            }
        )
    })?;
    if let Some(r) = v.get("refusal").filter(|r| !r.is_null()) {
        anyhow::bail!(
            "{tool} refused: {} ({})",
            r["message"].as_str().unwrap_or(""),
            r["code"].as_str().unwrap_or("refused")
        );
    }
    Ok(v)
}

/// Windows on the agent's workspace (what it may touch).
fn agent_windows(home: &Path, agent: &str) -> anyhow::Result<(i64, Vec<hypr::Client>)> {
    let space = agent::space_of(home, agent)?;
    let windows = hypr::clients()?
        .into_iter()
        .filter(|c| c.mapped && c.workspace.id == space.workspace)
        .collect();
    Ok((space.workspace, windows))
}

/// The agent's windows, for picking one by address.
pub fn list_windows(home: &Path, agent: &str) -> anyhow::Result<Value> {
    let (ws, windows) = agent_windows(home, agent)?;
    Ok(
        json!({"workspace": ws, "windows": windows.iter().map(|c| json!({"address": c.address, "class": c.class, "title": c.title, "pid": c.pid})).collect::<Vec<_>>()}),
    )
}

/// Start an app on the agent's workspace (a command, or a .desktop id).
pub fn open_app(home: &Path, agent: &str, command: &str) -> anyhow::Result<Value> {
    let (ws, _) = agent_windows(home, agent)?;
    // Every window that exists now, anywhere: single-instance apps (Nautilus,
    // Chromium) open a new window from their running process, which ignores
    // the launch's workspace rule and lands on the user's workspace.
    let before = hypr::clients()?;
    anyhow::ensure!(
        !command.trim().is_empty() && !command.contains('\n'),
        "a command is required"
    );
    // A .desktop id runs through gtk-launch; anything else is a command line
    // (the agent's own: it runs with your user's rights, like a terminal).
    let line = if command.ends_with(".desktop") {
        format!("gtk-launch {}", command.trim_end_matches(".desktop"))
    } else {
        command.to_string()
    };
    hypr::dispatch(&format!(
        "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
        hypr::lua_string(&format!("uwsm-app -- {line}")),
        hypr::lua_string(&format!("{ws} silent"))
    ))?;
    let old: std::collections::HashSet<String> = before.iter().map(|c| c.address.clone()).collect();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        if let Some(w) = hypr::clients()?
            .into_iter()
            .find(|c| c.mapped && !old.contains(&c.address))
        {
            if w.workspace.id != ws {
                // Send it where it belongs, without following it.
                hypr::dispatch(&format!(
                    "hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})",
                    hypr::lua_string(&ws.to_string()),
                    hypr::lua_string(&format!("address:{}", w.address))
                ))?;
            }
            return Ok(
                json!({"address": w.address, "class": w.class, "title": w.title, "pid": w.pid, "workspace": ws}),
            );
        }
        anyhow::ensure!(
            std::time::Instant::now() < end,
            "no window appeared within 15s"
        );
    }
}

/// The driver's id for a Hyprland window the agent owns. Remembered per
/// address: asking the driver again (list_windows) would invalidate the
/// element tokens from the last app_read.
fn target(home: &Path, agent: &str, address: &str) -> anyhow::Result<(i64, u64)> {
    let (ws, windows) = agent_windows(home, agent)?;
    let cache = home.join(".local/state/omaspace/driver-windows.json");
    let mut known: std::collections::BTreeMap<String, (i64, u64)> = std::fs::read(&cache)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if let Some(&(pid, id)) = known.get(address)
        && windows.iter().any(|c| c.address == address && c.pid == pid)
    {
        return Ok((pid, id));
    }
    let r = target_lookup(address, ws, &windows)?;
    known.retain(|a, _| windows.iter().any(|c| &c.address == a));
    known.insert(address.to_string(), r);
    let _ = std::fs::write(&cache, serde_json::to_vec(&known)?);
    Ok(r)
}

fn target_lookup(address: &str, ws: i64, windows: &[hypr::Client]) -> anyhow::Result<(i64, u64)> {
    let w = windows
        .iter()
        .find(|c| c.address == address)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "window {address} isn't on your workspace {ws}; you may only use your own windows"
            )
        })?;
    // cua-driver numbers windows its own way: find this one among the app's
    // windows by its geometry (Hyprland's at/size).
    let listed = driver_call("list_windows", &json!({"pid": w.pid}))?;
    let windows = listed["windows"].as_array().cloned().unwrap_or_default();
    let same = |d: &Value| {
        d["x"].as_i64() == Some(w.at[0])
            && d["y"].as_i64() == Some(w.at[1])
            && d["width"].as_i64() == Some(w.size[0])
            && d["height"].as_i64() == Some(w.size[1])
    };
    let found = windows
        .iter()
        .find(|d| same(d))
        .or_else(|| (windows.len() == 1).then(|| &windows[0]));
    let id = found
        .and_then(|d| d["window_id"].as_u64())
        .ok_or_else(|| anyhow::anyhow!("cua-driver doesn't see window {address} ({})", w.class))?;
    Ok((w.pid, id))
}

/// The window's accessibility tree (buttons, fields, text, with tokens to
/// act on) and a screenshot.
pub fn app_read(
    home: &Path,
    agent: &str,
    address: &str,
    screenshot: bool,
) -> anyhow::Result<Value> {
    let (pid, window_id) = target(home, agent, address)?;
    let mut v = driver_call_in(
        Some(&sess(agent)),
        "get_window_state",
        &json!({"pid": pid, "window_id": window_id}),
    )?;
    if !screenshot && let Some(o) = v.as_object_mut() {
        o.remove("screenshot_png_b64");
    }
    // The tree as markdown is what an agent reads; elements carry the tokens.
    let elements: Vec<Value> = v["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["label"].is_string() || e["value"].is_string() || e["actions"].as_array().is_some_and(|a| !a.is_empty()))
        .map(|e| json!({"token": e["element_token"], "role": e["role"], "label": e["label"], "value": e["value"], "actions": e["actions"]}))
        .collect();
    Ok(json!({
        "title": v["window_title"], "snapshot": v["snapshot_id"], "elements": elements,
        "screenshot_png_b64": v.get("screenshot_png_b64").cloned().unwrap_or(Value::Null),
    }))
}

/// Click an element (by token from app_read), or a point in the window.
pub fn app_click(
    home: &Path,
    agent: &str,
    address: &str,
    token: Option<&str>,
    at: Option<(f64, f64)>,
) -> anyhow::Result<Value> {
    let (pid, window_id) = target(home, agent, address)?;
    let mut args = json!({"pid": pid, "window_id": window_id});
    match (token, at) {
        (Some(t), _) => args["element_token"] = json!(t),
        (None, Some((x, y))) => {
            args["x"] = json!(x);
            args["y"] = json!(y);
        }
        _ => anyhow::bail!("give an element token or x and y"),
    }
    let v = driver_call_in(Some(&sess(agent)), "click", &args)?;
    Ok(json!({"clicked": true, "route": v["route"]}))
}

/// Set a field's value (accessibility SetValue), by token.
pub fn app_set_value(
    home: &Path,
    agent: &str,
    address: &str,
    token: &str,
    value: &str,
) -> anyhow::Result<Value> {
    let (pid, window_id) = target(home, agent, address)?;
    let v = driver_call_in(
        Some(&sess(agent)),
        "set_value",
        &json!({"pid": pid, "window_id": window_id, "element_token": token, "value": value}),
    )?;
    Ok(json!({"set": true, "route": v["route"]}))
}

/// Type text / press keys into the window (cua's Hyprland plugin: only apps
/// in ~/.config/cua-driver/qualified-apps, and not while you're using them).
pub fn app_type(home: &Path, agent: &str, address: &str, text: &str) -> anyhow::Result<Value> {
    let (pid, window_id) = target(home, agent, address)?;
    let v = driver_call_in(
        Some(&sess(agent)),
        "type_text",
        &json!({"pid": pid, "window_id": window_id, "text": text}),
    )?;
    Ok(json!({"typed": true, "route": v["route"]}))
}

pub fn app_key(home: &Path, agent: &str, address: &str, keys: &[String]) -> anyhow::Result<Value> {
    let (pid, window_id) = target(home, agent, address)?;
    let v = if keys.len() == 1 {
        driver_call_in(
            Some(&sess(agent)),
            "press_key",
            &json!({"pid": pid, "window_id": window_id, "key": keys[0]}),
        )?
    } else {
        driver_call_in(
            Some(&sess(agent)),
            "hotkey",
            &json!({"pid": pid, "window_id": window_id, "keys": keys}),
        )?
    };
    Ok(json!({"pressed": true, "route": v["route"]}))
}

/// Close one of the agent's windows.
pub fn close_window(home: &Path, agent: &str, address: &str) -> anyhow::Result<()> {
    target(home, agent, address)?;
    hypr::dispatch(&format!(
        "hl.dsp.window.close({{ window = {} }})",
        hypr::lua_string(&format!("address:{address}"))
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn agent_sessions_are_namespaced_and_tmux_safe() {
        assert_eq!(super::session("Claude Code", "build"), "Claude-Code--build");
        assert_eq!(super::session("a.b", "x:y"), "a-b--x-y");
    }
}
