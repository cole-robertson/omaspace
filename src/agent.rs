//! Agent spaces: an agent works on its own Omarchy workspace on this machine,
//! next to you, and never takes your focus or cursor. You swipe to its
//! workspace to watch; it asks for help when it needs you.
//!
//! Its browsers are Chromiums in their own throwaway profile, seeded with your
//! sign-ins and site data, on the agent's workspace, driven over the DevTools
//! protocol (cdp.rs), so no keyboard or pointer events ever reach your session.
//! Their window class is `omaspace-agent`, which a Hyprland rule keeps from
//! "activating" (Omarchy focuses windows that ask for attention).
//!
//! State is one JSON file, so the daemon, the live view and every agent's MCP
//! server (each its own process) see the same spaces.

use crate::{cdp, hypr, omarchy, profile};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The window class agent browsers run under (see the Hyprland rule).
pub const CLASS: &str = "omaspace-agent";
/// Workspaces an agent may claim, highest first: they sit out of your way.
const SPACE_RANGE: std::ops::RangeInclusive<i64> = 5..=9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Space {
    pub workspace: i64,
    pub agent: String,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub status: String,
    /// What the agent needs from you, while it waits.
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub browsers: Vec<AgentBrowser>,
    pub claimed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentBrowser {
    pub id: String,
    /// DevTools, relayed by `omaspace cdp-broker` (never a TCP port).
    #[serde(default)]
    pub socket: PathBuf,
    pub pid: u32,
    pub data_dir: PathBuf,
    /// Set when it was handed over from your browser: where it came from.
    #[serde(default)]
    pub handed_from: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    spaces: Vec<Space>,
    /// Someone took over from the live view: agents' actions are refused
    /// until they hand back. Shared on disk because each agent has its own
    /// MCP server process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    taken_over_by: Option<String>,
}

/// Pause (`Some(name)`) or resume (`None`) every agent on this machine.
/// Taking over answers the agents' requests for help: a person is on it, so
/// the requests are cleared rather than shown again after hand back.
pub fn set_taken_over(home: &Path, by: Option<&str>) -> anyhow::Result<()> {
    with_store(home, |store| {
        store.taken_over_by = by.map(|b| b.chars().take(80).collect());
        if by.is_some() {
            for s in &mut store.spaces {
                if s.help.take().is_some() {
                    s.status = "you took over".into();
                }
            }
        }
        Ok(())
    })
}

/// Who has taken over, if anyone (agents are paused meanwhile).
pub fn taken_over_by(home: &Path) -> Option<String> {
    std::fs::read(store_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice::<Store>(&b).ok())
        .and_then(|s| s.taken_over_by)
}

fn state_dir(home: &Path) -> PathBuf {
    home.join(".local/state/omaspace")
}

fn store_path(home: &Path) -> PathBuf {
    state_dir(home).join("agents.json")
}

/// Read, change and write the store under an exclusive file lock.
fn with_store<T>(
    home: &Path,
    f: impl FnOnce(&mut Store) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    use std::os::fd::AsRawFd;
    std::fs::create_dir_all(state_dir(home))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir(home).join("agents.lock"))?;
    unsafe { flock(lock.as_raw_fd(), 2) };
    let mut store: Store = std::fs::read(store_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // A space whose browsers all died is still the agent's: only release frees it.
    let out = f(&mut store)?;
    let tmp = store_path(home).with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&store)?)?;
    std::fs::rename(tmp, store_path(home))?;
    Ok(out)
}

unsafe extern "C" {
    fn flock(fd: i32, op: i32) -> i32;
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn list(home: &Path) -> Vec<Space> {
    with_store(home, |s| Ok(s.spaces.clone())).unwrap_or_default()
}

/// Claim a workspace for `agent`: its existing one, or the highest free one
/// in 5–9 that has no windows and isn't the one you're on.
pub fn claim(home: &Path, agent: &str, task: &str) -> anyhow::Result<Space> {
    anyhow::ensure!(
        !agent.trim().is_empty() && agent.len() <= 40,
        "an agent name (up to 40 characters) is required"
    );
    let clients = hypr::clients()?;
    let active = hypr::active_workspace().unwrap_or(1);
    with_store(home, |store| {
        if let Some(s) = store.spaces.iter_mut().find(|s| s.agent == agent) {
            if !task.is_empty() {
                s.task = task.to_string();
            }
            return Ok(s.clone());
        }
        let busy = |ws: i64| {
            ws == active
                || store.spaces.iter().any(|s| s.workspace == ws)
                || clients.iter().any(|c| c.workspace.id == ws)
        };
        let ws = SPACE_RANGE
            .rev()
            .find(|ws| !busy(*ws))
            .ok_or_else(|| anyhow::anyhow!("no free workspace in 5–9 for an agent"))?;
        let space = Space {
            workspace: ws,
            agent: agent.into(),
            task: task.into(),
            status: "starting".into(),
            help: None,
            browsers: vec![],
            claimed_at: now(),
        };
        store.spaces.push(space.clone());
        Ok(space)
    })
}

pub fn update(
    home: &Path,
    agent: &str,
    status: Option<&str>,
    help: Option<Option<&str>>,
) -> anyhow::Result<Space> {
    with_store(home, |store| {
        let s = store
            .spaces
            .iter_mut()
            .find(|s| s.agent == agent)
            .ok_or_else(|| anyhow::anyhow!("{agent} has no space; claim one first"))?;
        if let Some(st) = status {
            s.status = st.chars().take(160).collect();
        }
        if let Some(h) = help {
            s.help = h.map(|t| t.chars().take(300).collect());
        }
        Ok(s.clone())
    })
}

pub fn space_of(home: &Path, agent: &str) -> anyhow::Result<Space> {
    list(home)
        .into_iter()
        .find(|s| s.agent == agent)
        .ok_or_else(|| anyhow::anyhow!("{agent} has no space; claim one first"))
}

/// Release: close its browsers, delete their profiles, free the workspace.
pub fn release(home: &Path, agent: &str) -> anyhow::Result<()> {
    let space = space_of(home, agent)?;
    for b in &space.browsers {
        stop_browser(b);
    }
    // Its terminals, and any other windows it left on its workspace.
    crate::apps::close_terminals(agent);
    if let Ok(clients) = hypr::clients() {
        for c in clients.iter().filter(|c| c.workspace.id == space.workspace) {
            let _ = hypr::dispatch(&format!(
                "hl.dsp.window.close({{ window = {} }})",
                hypr::lua_string(&format!("address:{}", c.address))
            ));
        }
    }
    with_store(home, |store| {
        store.spaces.retain(|s| s.agent != agent);
        Ok(())
    })
}

// ---- browsers ---------------------------------------------------------------

/// Your Chromium (Omarchy's default browser) and its running profile.
fn user_browser(home: &Path) -> anyhow::Result<(omarchy::Browser, PathBuf)> {
    let browsers = omarchy::browsers(home);
    let b = omarchy::launchable_browser(&browsers, "chromium")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no Chromium-family browser installed"))?;
    let dir = omarchy::running_data_dir(&b, home);
    Ok((b, dir))
}

/// Seed a fresh profile with all your sign-ins and site data, then start it on
/// the agent's workspace. `urls` open as its tabs (blank page if none).
pub fn open_browser(
    home: &Path,
    agent: &str,
    urls: &[String],
    signed_in: bool,
    handed_from: Option<String>,
) -> anyhow::Result<AgentBrowser> {
    let space = space_of(home, agent)?;
    let (browser, your_dir) = user_browser(home)?;
    let id = format!("{}-{}", now(), std::process::id() % 10000);
    let data_dir = state_dir(home).join("agent-browsers").join(&id);
    std::fs::create_dir_all(data_dir.join("Default"))?;
    if signed_in {
        let data = profile::capture_all(&your_dir.join("Default"))?;
        profile::apply_all(&data_dir.join("Default"), &data)?;
    }
    // DevTools over a pipe the broker holds, relayed on a 0600 socket: with a
    // --remote-debugging-port any local user could read this profile's
    // cookies (a copy of your sign-ins) over loopback.
    let socket = cdp::socket_for(&id);
    let me = std::env::current_exe()?;
    let mut argv = vec![
        me.display().to_string(),
        "cdp-broker".into(),
        socket.display().to_string(),
        "--".into(),
        browser.binary.clone(),
        format!("--class={CLASS}"),
        format!("--user-data-dir={}", data_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--new-window".into(),
    ];
    if urls.is_empty() {
        argv.push("about:blank".into());
    } else {
        argv.extend(urls.iter().cloned());
    }
    ensure_window_rule();
    let cmd = crate::restore::shell_join(&argv);
    hypr::dispatch(&format!(
        "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
        hypr::lua_string(&cmd),
        hypr::lua_string(&format!("{} silent", space.workspace))
    ))?;
    // Wait for its DevTools, then find the main process by its profile.
    let cdp = cdp::Browser {
        socket: socket.clone(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !cdp.alive() {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the agent's browser didn't start"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let pid = profile::running_on(&data_dir, &omarchy_binary_name(&browser))
        .first()
        .copied()
        .unwrap_or(0);
    let ab = AgentBrowser {
        id,
        socket,
        pid,
        data_dir,
        handed_from,
    };
    with_store(home, |store| {
        if let Some(s) = store.spaces.iter_mut().find(|s| s.agent == agent) {
            s.browsers.push(ab.clone());
        }
        Ok(())
    })?;
    Ok(ab)
}

fn omarchy_binary_name(b: &omarchy::Browser) -> String {
    b.binary.rsplit('/').next().unwrap_or(&b.binary).to_string()
}

/// Agent browsers must never take focus: Omarchy sets focus_on_activate, and
/// a page click "activates" the window. Idempotent; also in `omaspace setup`.
pub fn ensure_window_rule() {
    ensure_window_rule_for(CLASS);
}

/// The same rule for another agent window class (terminals).
pub fn ensure_window_rule_for(class: &str) {
    let _ = hypr::eval(&window_rule_lua_for(class));
}

pub fn window_rule_lua_for(class: &str) -> String {
    format!(
        "hl.window_rule({{ match = {{ class = \"^({class})$\" }}, suppress_event = \"activate activatefocus\" }})"
    )
}

/// Stop a browser and delete its profile (it held a copy of your sign-ins).
fn stop_browser(b: &AgentBrowser) {
    if b.pid > 0 {
        unsafe { kill(b.pid as i32, 15) };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < end && Path::new(&format!("/proc/{}", b.pid)).exists() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if b.data_dir
        .components()
        .any(|c| c.as_os_str() == "agent-browsers")
    {
        let _ = std::fs::remove_dir_all(&b.data_dir);
    }
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// The agent's browser (its first, or by id).
pub fn browser(home: &Path, agent: &str, id: Option<&str>) -> anyhow::Result<cdp::Browser> {
    let space = space_of(home, agent)?;
    let b = space
        .browsers
        .iter()
        .find(|b| id.is_none_or(|i| b.id == i))
        .ok_or_else(|| anyhow::anyhow!("{agent} has no browser; open one first"))?;
    Ok(cdp::Browser {
        socket: b.socket.clone(),
    })
}

// ---- hand over / take back ----------------------------------------------------

/// Hand one of your browser windows to an agent: its tabs (with all your
/// sign-ins) open in a new agent browser on the agent's workspace, and your
/// window closes.
pub fn hand_over(home: &Path, agent: &str, window: &str) -> anyhow::Result<AgentBrowser> {
    let (snapshot, _) = crate::capture::capture_with(
        crate::capture::Scope::Window(window.into()),
        "here",
        home,
        &[],
    )?;
    let urls: Vec<String> = snapshot
        .workspaces
        .iter()
        .flat_map(|w| &w.windows)
        .find_map(|w| match &w.content {
            crate::snapshot::Content::Browser { urls, .. } => Some(urls.clone()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("that window isn't a browser window"))?;
    let b = open_browser(home, agent, &urls, true, Some(window.into()))?;
    hypr::dispatch(&format!(
        "hl.dsp.window.close({{ window = {} }})",
        hypr::lua_string(&format!("address:{window}"))
    ))?;
    Ok(b)
}

/// Take an agent's browser back: its open tabs come back to your browser on
/// your current workspace, with any sign-ins it picked up, and the agent's
/// browser and its profile are deleted.
pub fn take_back(home: &Path, agent: &str, id: Option<&str>) -> anyhow::Result<Vec<String>> {
    let space = space_of(home, agent)?;
    let b = space
        .browsers
        .iter()
        .find(|b| id.is_none_or(|i| b.id == i))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{agent} has no browser"))?;
    let devtools = cdp::Browser {
        socket: b.socket.clone(),
    };
    let tabs: Vec<String> = devtools
        .tabs()
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.url)
        .filter(|u| !u.is_empty() && u != "about:blank")
        .collect();
    // Chromium writes cookies to disk lazily (every ~30s). Ask it to close
    // through DevTools and let it exit on its own: a clean shutdown flushes
    // the cookie store; a kill straight after would lose the newest sign-ins.
    let _ = devtools.close();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while b.pid > 0
        && std::time::Instant::now() < end
        && Path::new(&format!("/proc/{}", b.pid)).exists()
    {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    stop_browser_keep(&b);
    // Sign-ins it picked up go back into your profile (your browser restarts
    // briefly, as on a take-back from another machine).
    let (browser, your_dir) = user_browser(home)?;
    let data = profile::capture_all(&b.data_dir.join("Default"))?;
    let restarted = profile::close_browser(&your_dir, &omarchy_binary_name(&browser))?;
    let applied = profile::apply_all(&your_dir.join("Default"), &data)?;
    if restarted {
        profile::mark_clean_exit(&your_dir.join("Default"))?;
    }
    eprintln!(
        "agent: took back {agent}'s browser: {} cookie(s), {} storage item(s) into {}",
        applied.cookies,
        applied.local_storage,
        your_dir.display()
    );
    if b.data_dir
        .components()
        .any(|c| c.as_os_str() == "agent-browsers")
    {
        let _ = std::fs::remove_dir_all(&b.data_dir);
    }
    with_store(home, |store| {
        if let Some(s) = store.spaces.iter_mut().find(|s| s.agent == agent) {
            s.browsers.retain(|x| x.id != b.id);
        }
        Ok(())
    })?;
    let ws = hypr::active_workspace().unwrap_or(1);
    let mut argv = vec![
        browser.binary.clone(),
        format!("--user-data-dir={}", your_dir.display()),
        "--no-first-run".into(),
        "--new-window".into(),
    ];
    argv.extend(if tabs.is_empty() {
        vec!["about:blank".to_string()]
    } else {
        tabs.clone()
    });
    let cmd = crate::restore::shell_join(&argv);
    hypr::dispatch(&format!(
        "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
        hypr::lua_string(&cmd),
        hypr::lua_string(&format!("{ws} silent"))
    ))?;
    if restarted {
        eprintln!("agent: your browser was restarted to take the sign-ins");
    }
    Ok(tabs)
}

/// Stop the process but keep the profile (take_back still reads it).
fn stop_browser_keep(b: &AgentBrowser) {
    if b.pid > 0 {
        unsafe { kill(b.pid as i32, 15) };
        let end = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while std::time::Instant::now() < end && Path::new(&format!("/proc/{}", b.pid)).exists() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_rule_targets_only_agent_browsers() {
        let lua = window_rule_lua_for(CLASS);
        assert!(lua.contains("^(omaspace-agent)$"));
        assert!(lua.contains("activate activatefocus"));
    }

    #[test]
    fn store_round_trips_and_releases() {
        let home = tempfile::tempdir().unwrap();
        with_store(home.path(), |s| {
            s.spaces.push(Space {
                workspace: 9,
                agent: "a".into(),
                task: "t".into(),
                status: "".into(),
                help: None,
                browsers: vec![],
                claimed_at: 1,
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(list(home.path()).len(), 1);
        update(home.path(), "a", Some("working"), Some(Some("need a code"))).unwrap();
        let s = space_of(home.path(), "a").unwrap();
        assert_eq!(
            (s.status.as_str(), s.help.as_deref()),
            ("working", Some("need a code"))
        );
        release(home.path(), "a").unwrap();
        assert!(list(home.path()).is_empty());
    }
}

#[cfg(test)]
mod take_over {
    #[test]
    fn taking_over_answers_help_requests_and_handing_back_resumes() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        // A space as claim() leaves it (claim needs a live Hyprland).
        std::fs::create_dir_all(h.join(".local/state/omaspace")).unwrap();
        std::fs::write(
            h.join(".local/state/omaspace/agents.json"),
            r#"{"spaces":[{"workspace":9,"agent":"A","task":"book dinner","status":"","help":"which time?","browsers":[],"claimed_at":1}]}"#,
        )
        .unwrap();
        super::set_taken_over(h, Some("Phone")).unwrap();
        assert_eq!(super::taken_over_by(h).as_deref(), Some("Phone"));
        let s = super::space_of(h, "A").unwrap();
        assert_eq!(
            s.help, None,
            "a person is on it: the request doesn't come back after hand back"
        );
        assert_eq!(s.status, "you took over");
        super::set_taken_over(h, None).unwrap();
        assert_eq!(super::taken_over_by(h), None);
        assert_eq!(super::space_of(h, "A").unwrap().help, None);
    }
}

#[cfg(test)]
mod store_compat {
    #[test]
    fn a_store_from_the_port_version_still_loads() {
        let old = r#"{"spaces":[{"agent":"A","task":"t","workspace":9,"status":"s","claimed_at":1,"help":null,"browsers":[{"id":"x","port":34117,"pid":0,"data_dir":"/x/agent-browsers/x"}]}]}"#;
        let store: super::Store =
            serde_json::from_str(old).expect("old agents.json must still parse");
        assert_eq!(store.spaces[0].browsers[0].id, "x");
        assert!(
            store.spaces[0].browsers[0].socket.as_os_str().is_empty(),
            "no socket: it can't be driven, only released"
        );
    }
}
