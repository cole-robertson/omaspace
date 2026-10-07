//! `omaspace view`: an Omarchy-native remote desktop for browsers.
//!
//! Listens on a unix socket only the user can open
//! (`$XDG_RUNTIME_DIR/omaspace-view.sock`), published on the tailnet with
//! `tailscale serve --https=7788 unix:<socket>` (browsers need HTTPS for
//! WebCodecs). There is no TCP listener, so other users on this machine cannot
//! reach it and forge Tailscale Serve's identity headers. Every request must
//! carry the tailnet address Tailscale Serve forwards, and it must pass the
//! omaspace trust rule. Browser requests must also come from this view's own
//! origin, so a web page open on one of your devices cannot drive it.
//!
//! One WebSocket per viewer:
//! - server → client binary: `0x01 | u8 key | u64 pts_us | H.264 Annex B`
//! - server → client text:   `{"type":"state"|"cursors"|"help"|...}`
//! - client → server text:   input, Omarchy actions, `hello{name,kind}`
//!
//! Agents (cua-driver users) report their pointer and ask for help with
//! `POST /v1/view/agent` on the same listener.

use crate::hypr;
use crate::input::{self, Event};
use crate::stream::{Captures, Source};
use crate::tailnet::{self, Identity};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use tungstenite::{Message, WebSocket};

pub const PORT: u16 = 7788;
const PAGE: &str = include_str!("../web/view.html");

/// The view's socket: in the user's runtime directory (0700), and 0600 itself.
pub fn socket_path() -> PathBuf {
    crate::sock::runtime_dir().join("omaspace-view.sock")
}

#[derive(Clone, serde::Serialize)]
struct Participant {
    id: u64,
    name: String,
    kind: String,
    x: f64,
    y: f64,
    /// Agents report over HTTP with no disconnect, so their cursor expires.
    #[serde(skip)]
    seen: std::time::Instant,
}

/// An agent cursor with no report for this long is dropped.
const AGENT_CURSOR_TTL: std::time::Duration = std::time::Duration::from_secs(30);

struct Shared {
    me: Identity,
    /// This view's own origin, `https://<host>.<tailnet>:7788`.
    origin: String,
    captures: Captures,
    participants: Mutex<HashMap<u64, Participant>>,
    /// Text messages to every viewer (state, cursors, help).
    outboxes: Mutex<HashMap<u64, mpsc::Sender<String>>>,
    /// Input channels keyed by target output ("" = the primary monitor).
    input: Mutex<HashMap<String, mpsc::Sender<Event>>>,
    /// Phone screens: viewer id → (virtual output name, workspace it took).
    phones: Mutex<HashMap<u64, (String, i64)>>,
    /// A human took over: agent input is refused until hand back.
    taken_over: Mutex<Option<String>>,
    taken_by: Mutex<Option<u64>>,
    help: Mutex<Option<String>>,
    next_id: Mutex<u64>,
    output: (String, u32, u32),
}

pub fn serve() -> anyhow::Result<()> {
    let me = tailnet::me()?;
    let origin = view_origin(&tailnet::my_dns_name()?);
    let output = primary_output()?;
    remove_stale_phone_screens(&output.0);
    // A take-over belongs to a viewer, and a restart closes every viewer:
    // don't leave agents paused by a person who is no longer connected.
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    if crate::agent::taken_over_by(&home).is_some() {
        let _ = crate::agent::set_taken_over(&home, None);
    }
    let shared = Arc::new(Shared {
        me,
        origin,
        captures: Captures::default(),
        participants: Mutex::new(HashMap::new()),
        outboxes: Mutex::new(HashMap::new()),
        input: Mutex::new(HashMap::new()),
        phones: Mutex::new(HashMap::new()),
        taken_over: Mutex::new(None),
        taken_by: Mutex::new(None),
        help: Mutex::new(None),
        next_id: Mutex::new(1),
        output,
    });
    spawn_hyprland_events(shared.clone());
    // Agent spaces change in other processes (each agent's MCP server):
    // refresh viewers when agents.json does.
    let watch = shared.clone();
    std::thread::spawn(move || {
        let file = std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".local/state/omaspace/agents.json"));
        let mut last = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let stamp = file
                .as_ref()
                .and_then(|f| std::fs::metadata(f).ok())
                .and_then(|m| m.modified().ok());
            if stamp != last {
                if last.is_some() {
                    broadcast(&watch, json!({"type": "state_changed"}));
                }
                last = stamp;
            }
        }
    });
    // Expire agent cursors that stopped reporting.
    let sweep = shared.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let removed = {
                let mut list = sweep.participants.lock().unwrap();
                let before = list.len();
                list.retain(|_, p| p.kind != "agent" || p.seen.elapsed() < AGENT_CURSOR_TTL);
                before != list.len()
            };
            if removed {
                broadcast_cursors(&sweep);
            }
        }
    });
    let socket = socket_path();
    let listener = crate::sock::bind_private(&socket)?;
    eprintln!(
        "omaspace view on {} as {} (publish with: tailscale serve --bg --https={PORT} unix:{})",
        socket.display(),
        shared.origin,
        socket.display()
    );
    for stream in listener.incoming().flatten() {
        let shared = shared.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, shared) {
                eprintln!("view: {e}");
            }
        });
    }
    Ok(())
}

/// `alpha.tail1234.ts.net.` → `https://alpha.tail1234.ts.net:7788`.
fn view_origin(dns_name: &str) -> String {
    format!(
        "https://{}:{PORT}",
        dns_name.trim_end_matches('.').to_ascii_lowercase()
    )
}

/// Browser requests must come from this view's own page. A WebSocket or a
/// simple POST from any other site would otherwise be sent with your device's
/// tailnet identity (Tailscale Serve forwards it) and pass the trust rule.
/// Requests with no Origin header are not from a web page (curl, agents) and
/// are allowed: the identity check still applies to them.
fn origin_allowed(headers: &HashMap<String, String>, own: &str) -> bool {
    match headers.get("origin") {
        None => true,
        Some(o) => o.eq_ignore_ascii_case(own),
    }
}

/// The caller's tailnet address from Tailscale Serve's `X-Forwarded-For`.
/// Serve replaces any client-sent value (checked 2026-10-05: a forged header
/// through Serve still identified the real device); if a proxy ever appends
/// instead, the last entry is still the one the nearest hop vouches for.
fn forwarded_ip(headers: &HashMap<String, String>) -> anyhow::Result<&str> {
    let forwarded = headers
        .get("x-forwarded-for")
        .ok_or_else(|| anyhow::anyhow!("no X-Forwarded-For"))?;
    let ip = forwarded.rsplit(',').next().unwrap_or("").trim();
    anyhow::ensure!(
        ip.starts_with("100.") || ip.starts_with("fd7a:"),
        "not a tailnet address"
    );
    Ok(ip)
}

fn primary_output() -> anyhow::Result<(String, u32, u32)> {
    let monitors: Value = serde_json::from_str(&hypr::ctl(
        &["-j", "monitors"],
        std::time::Duration::from_secs(3),
    )?)?;
    let m = monitors
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("no monitor"))?;
    Ok((
        m["name"].as_str().unwrap_or("").to_string(),
        m["width"].as_u64().unwrap_or(1920) as u32,
        m["height"].as_u64().unwrap_or(1080) as u32,
    ))
}

/// A request head: method, path, lowercased headers, and any body bytes
/// already read past it.
type Head = (String, String, HashMap<String, String>, Vec<u8>);

/// Read the HTTP request head. Returns any bytes already read past it (the
/// start of a POST body sent in the same packet).
fn read_head(stream: &mut UnixStream) -> anyhow::Result<Head> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let buffered = reader.buffer().to_vec();
    anyhow::ensure!(
        buffered.is_empty() || method == "POST" || method == "PUT",
        "unexpected pipelined data"
    );
    Ok((method, path, headers, buffered))
}

/// The caller's identity: only this user and Tailscale Serve (root) can open
/// the socket, and the forwarded tailnet address must pass whois + trust.
fn caller(headers: &HashMap<String, String>, me: &Identity) -> anyhow::Result<Identity> {
    let ip = forwarded_ip(headers)?;
    let id = tailnet::whois(&format!("{ip}:1"))?;
    anyhow::ensure!(
        tailnet::trusted(me, &id),
        "{} is not one of your devices",
        id.name
    );
    Ok(id)
}

fn http(mut stream: UnixStream, status: &str, ctype: &str, body: &[u8]) -> anyhow::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

/// 2 minutes of 16 kHz 16-bit mono, plus the header.
const DICTATION_MAX_BYTES: usize = 16_000 * 2 * 120 + 1024;

/// WAV → text with voxtype (Omarchy's dictation, `omarchy-voxtype-install`).
fn transcribe(wav: &[u8]) -> anyhow::Result<String> {
    anyhow::ensure!(
        wav.len() > 44 && &wav[..4] == b"RIFF" && &wav[8..12] == b"WAVE",
        "not a WAV file"
    );
    let voxtype = crate::omarchy::which("voxtype").ok_or_else(|| {
        anyhow::anyhow!("dictation isn't set up on this machine (run omarchy-voxtype-install)")
    })?;
    let mut file = tempfile::Builder::new()
        .prefix("omaspace-dictate-")
        .suffix(".wav")
        .tempfile()?;
    file.write_all(wav)?;
    let out = std::process::Command::new(voxtype)
        .arg("transcribe")
        .arg(file.path())
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "voxtype: {}",
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("failed")
    );
    // voxtype prints logs, a blank line, then the text: keep the last line.
    let text = String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(text)
}

/// JSON another machine's view page may read (its origin is another host on
/// the same tailnet; the data is presence only, no screen or input). Only an
/// omaspace view origin on this tailnet is allowed to read it, not `*`.
fn http_cors(mut stream: UnixStream, body: &str, allow_origin: Option<&str>) -> anyhow::Result<()> {
    let cors = allow_origin
        .map(|o| format!("Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{cors}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

/// `https://<one label>.<this tailnet>:7788`: another machine's view page.
fn presence_origin_allowed(origin: &str, own: &str) -> bool {
    let Some(tailnet) = own
        .strip_prefix("https://")
        .and_then(|h| h.split_once('.'))
        .map(|(_, rest)| rest)
    else {
        return false;
    };
    let Some(host) = origin
        .to_ascii_lowercase()
        .strip_prefix("https://")
        .map(str::to_string)
    else {
        return false;
    };
    match host.split_once('.') {
        Some((label, rest)) => {
            !label.is_empty()
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && rest == tailnet
        }
        None => false,
    }
}

/// This machine plus every Omarchy desktop on your tailnet running omaspace
/// (the same discovery as `omaspace peers`), each with its live view URL
/// (https://<host>.<tailnet>:7788). Whether that view is published is up to
/// the page to find out (it asks each one's /v1/view/presence).
fn machines(shared: &Shared) -> Value {
    let suffix = tailnet::magic_dns_suffix().unwrap_or_default();
    let url = |host: &str| format!("https://{host}.{suffix}:{PORT}/");
    let mut list = vec![json!({"name": shared.me.name, "here": true, "url": url(&shared.me.name)})];
    for p in crate::client::peers()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.desktop)
    {
        list.push(json!({"name": p.name, "here": false, "url": url(&p.name)}));
    }
    json!({"machines": list})
}

/// Like `http`, for a static asset the browser may keep for a day.
fn http_cached(mut stream: UnixStream, ctype: &str, body: &[u8]) -> anyhow::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: max-age=86400\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

/// The page in this machine's Omarchy theme: colors.toml (the file the
/// Omarchy shell itself reads) becomes CSS variables on :root.
fn page() -> String {
    let theme = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/state/omarchy/current/theme/colors.toml"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    PAGE.replace("/*OMARCHY-THEME*/", &theme_css(&theme))
}

/// `accent = "#7aa2f7"` lines → `--accent:#7aa2f7;` (only #rrggbb values).
fn theme_css(colors_toml: &str) -> String {
    let mut css = String::new();
    for line in colors_toml.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        let hex = value.len() == 7
            && value.starts_with('#')
            && value[1..].chars().all(|c| c.is_ascii_hexdigit());
        if hex && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            css.push_str(&format!("--t-{}:{value};", key.replace('_', "-")));
        }
    }
    if css.is_empty() {
        css
    } else {
        format!(":root{{{css}}}")
    }
}

/// A color from this machine's Omarchy theme, or `fallback`.
fn theme_color(key: &str, fallback: &str) -> String {
    let toml = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/state/omarchy/current/theme/colors.toml"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    toml.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .filter(|v| {
            v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| fallback.to_string())
}

/// Omarchy's open-bracket square (the shape of /usr/share/omarchy/icon.png)
/// in the theme accent, on the theme background, rounded for a home screen.
fn icon_svg() -> String {
    let (bg, fg) = (
        theme_color("background", "#1a1b26"),
        theme_color("accent", "#7aa2f7"),
    );
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" rx="112" fill="{bg}"/><g transform="translate(106 106)" fill="{fg}"><path d="M0 0h140v40H40v80H0zM180 0h120v300H180v-40h80V40h-80zM0 160h40v100h100v40H0z"/></g></svg>"#
    )
}

/// 180px PNG of `icon_svg` via ImageMagick (shipped with Omarchy), cached.
fn icon_png() -> Option<Vec<u8>> {
    static PNG: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    PNG.get_or_init(|| {
        let mut child = std::process::Command::new("magick")
            .args([
                "-background",
                "none",
                "-density",
                "96",
                "svg:-",
                "-resize",
                "180x180",
                "png:-",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(icon_svg().as_bytes()).ok()?;
        let out = child.wait_with_output().ok()?;
        (out.status.success() && out.stdout.starts_with(b"\x89PNG")).then_some(out.stdout)
    })
    .clone()
}

/// Add-to-home-screen manifest: opens full screen, named after the machine,
/// in its theme colors.
fn manifest(name: &str) -> String {
    let bg = theme_color("dark_background", "#16161e");
    json!({
        "name": format!("{name} · omaspace"), "short_name": name, "start_url": "/", "scope": "/", "display": "standalone",
        "orientation": "any", "background_color": bg, "theme_color": bg,
        "icons": [{"src": "/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any"}, {"src": "/icon.png", "sizes": "180x180", "type": "image/png"}],
    })
    .to_string()
}

fn handle(mut stream: UnixStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let (method, path, headers, early) = read_head(&mut stream)?;
    let who = match caller(&headers, &shared.me) {
        Ok(id) => id,
        Err(e) => {
            return http(
                stream,
                "403 Forbidden",
                "text/plain",
                e.to_string().as_bytes(),
            );
        }
    };
    let route = path.split('?').next().unwrap_or("");
    // Presence is the one cross-origin read (another of your machines' view
    // pages asks for it); everything else, above all /ws, is same-origin only.
    if route != "/v1/view/presence" && !origin_allowed(&headers, &shared.origin) {
        return http(
            stream,
            "403 Forbidden",
            "text/plain",
            b"cross-origin request refused",
        );
    }
    match (method.as_str(), route) {
        ("GET", "/") => http(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            page().as_bytes(),
        ),
        // Omarchy's own UI font, so a phone renders the same glyphs (icons are
        // Nerd Font code points). Read from the system font directory only.
        ("GET", "/font.ttf") => {
            match std::fs::read("/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf") {
                Ok(font) => http_cached(stream, "font/ttf", &font),
                Err(_) => http(
                    stream,
                    "404 Not Found",
                    "text/plain",
                    b"no JetBrainsMono Nerd Font here",
                ),
            }
        }
        // Your other machines with a live view, for the machine switcher.
        ("GET", "/v1/view/machines") => http(
            stream,
            "200 OK",
            "application/json",
            machines(&shared).to_string().as_bytes(),
        ),
        // Who is here: humans watching and agents working (for the switcher).
        ("GET", "/v1/view/presence") => {
            let list = shared.participants.lock().unwrap();
            let agents: Vec<_> = list
                .values()
                .filter(|p| p.kind == "agent")
                .map(|p| p.name.clone())
                .collect();
            let watching = list.values().filter(|p| p.kind == "human").count();
            let reply = json!({"name": shared.me.name, "agents": agents, "watching": watching, "help": *shared.help.lock().unwrap()});
            drop(list);
            let allow = headers
                .get("origin")
                .filter(|o| presence_origin_allowed(o, &shared.origin));
            http_cors(stream, &reply.to_string(), allow.map(String::as_str))
        }
        // Home-screen icon: Omarchy's bracket mark in this machine's accent.
        ("GET", "/icon.svg") => http_cached(stream, "image/svg+xml", icon_svg().as_bytes()),
        // iOS home screens want a PNG: rendered from the SVG once per run.
        ("GET", "/icon.png") => match icon_png() {
            Some(png) => http_cached(stream, "image/png", &png),
            None => http(
                stream,
                "404 Not Found",
                "text/plain",
                b"no icon renderer (magick)",
            ),
        },
        ("GET", "/manifest.webmanifest") => http(
            stream,
            "200 OK",
            "application/manifest+json",
            manifest(&shared.me.name).as_bytes(),
        ),
        ("GET", "/ws") => {
            // Re-run the handshake on a fresh read of the request.
            let key = headers
                .get("sec-websocket-key")
                .cloned()
                .unwrap_or_default();
            let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
            write!(
                stream,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            )?;
            // A viewer whose link stalls can't buffer without bound: past
            // 8 MiB of unsent video, sends fail and frames are skipped
            // until the next keyframe.
            let mut config = tungstenite::protocol::WebSocketConfig::default();
            config.max_write_buffer_size = 8 << 20;
            let ws = WebSocket::from_raw_socket(
                stream,
                tungstenite::protocol::Role::Server,
                Some(config),
            );
            viewer(ws, shared, who, &path)
        }
        ("POST", "/v1/view/agent") => {
            let len: usize = headers
                .get("content-length")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0)
                .min(65536);
            let mut body = early;
            body.truncate(len);
            let mut rest = vec![0; len - body.len()];
            stream.read_exact(&mut rest)?;
            body.extend(rest);
            let reply = agent_report(&shared, &who, &serde_json::from_slice(&body)?);
            http(
                stream,
                "200 OK",
                "application/json",
                reply.to_string().as_bytes(),
            )
        }
        // Hold-to-talk: a 16 kHz mono WAV from the phone, transcribed here by
        // Omarchy's dictation tool (voxtype, local Whisper). Text comes back;
        // the viewer types it into the focused window.
        ("POST", "/v1/view/dictate") => {
            let len: usize = headers
                .get("content-length")
                .and_then(|l| l.parse().ok())
                .unwrap_or(0);
            if len == 0 || len > DICTATION_MAX_BYTES {
                return http(
                    stream,
                    "413 Payload Too Large",
                    "application/json",
                    json!({"error": "a clip is 1 byte to 2 minutes of audio"})
                        .to_string()
                        .as_bytes(),
                );
            }
            let mut body = early;
            body.truncate(len);
            let mut rest = vec![0; len - body.len()];
            stream.read_exact(&mut rest)?;
            body.extend(rest);
            let reply = match transcribe(&body) {
                Ok(text) => json!({"text": text}),
                Err(e) => json!({"error": e.to_string()}),
            };
            http(
                stream,
                "200 OK",
                "application/json",
                reply.to_string().as_bytes(),
            )
        }
        // Files on this machine, for the viewer (drag and drop, phone panel).
        (_, p) if p.starts_with("/v1/files/") => {
            view_files(stream, &method, &path, &headers, early)
        }
        _ => http(stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// The viewer's file endpoints: same rules as the daemon's (inside $HOME, no
/// hidden top-level folders, verified commit), served on the view port so a
/// phone needs only the one tailnet connection.
fn view_files(
    mut stream: UnixStream,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    early: Vec<u8>,
) -> anyhow::Result<()> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let (route, q) = path.split_once('?').unwrap_or((path, ""));
    let param = |k: &str| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
            .map(pct)
    };
    let result: anyhow::Result<Value> = (|| {
        let target = crate::xfer::resolve(
            &home,
            &param("path").unwrap_or_else(|| "~/Downloads".into()),
        )?;
        let portable = |p: &std::path::Path| crate::snapshot::to_portable(p, &home);
        Ok(match (method, route) {
            ("GET", "/v1/files/list") => {
                std::fs::create_dir_all(&target).ok();
                json!({ "path": portable(&target), "entries": crate::xfer::list(&target)? })
            }
            ("GET", "/v1/files/get") => {
                anyhow::ensure!(target.is_file(), "not a file");
                let len = std::fs::metadata(&target)?.len();
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().replace('"', ""))
                    .unwrap_or_default();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
                )?;
                crate::xfer::stream_file(&target, 0, &mut stream)?;
                return Ok(Value::Null);
            }
            ("PUT", "/v1/files/put") => {
                let offset: u64 = param("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
                let len: u64 = headers
                    .get("content-length")
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(0);
                anyhow::ensure!(len <= crate::xfer::CHUNK as u64, "chunk too large");
                let mut body = std::io::Read::chain(
                    &early[..early.len().min(len as usize)],
                    (&stream).take(len.saturating_sub(early.len() as u64)),
                );
                json!({ "received": crate::xfer::put_chunk(&target, offset, &mut body)? })
            }
            ("POST", "/v1/files/commit") => {
                let size: u64 = param("size").and_then(|v| v.parse().ok()).unwrap_or(0);
                let landed = crate::xfer::commit(
                    &target,
                    size,
                    &param("sha256").unwrap_or_default(),
                    false,
                )?;
                let _ = std::process::Command::new("notify-send")
                    .args([
                        "omaspace",
                        &format!(
                            "Received {}",
                            landed
                                .file_name()
                                .map(|n| n.to_string_lossy())
                                .unwrap_or_default()
                        ),
                    ])
                    .status();
                json!({ "path": portable(&landed) })
            }
            _ => anyhow::bail!("unknown files endpoint"),
        })
    })();
    match result {
        Ok(Value::Null) => Ok(()),
        Ok(v) => http(
            stream,
            "200 OK",
            "application/json",
            v.to_string().as_bytes(),
        ),
        Err(e) => http(
            stream,
            "400 Bad Request",
            "application/json",
            json!({ "error": e.to_string() }).to_string().as_bytes(),
        ),
    }
}

fn pct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An agent (driving with cua-driver) reports its pointer, or asks for help.
fn agent_report(shared: &Shared, who: &Identity, body: &Value) -> Value {
    if let Some(by) = shared.taken_over.lock().unwrap().clone() {
        return json!({"ok": false, "paused": true, "by": by});
    }
    let id = 1_000_000 + (who.user_id.unsigned_abs() % 1000);
    let name = body["name"].as_str().unwrap_or("agent").to_string();
    if let (Some(x), Some(y)) = (body["x"].as_f64(), body["y"].as_f64()) {
        shared.participants.lock().unwrap().insert(
            id,
            Participant {
                id,
                name: name.clone(),
                kind: "agent".into(),
                x,
                y,
                seen: std::time::Instant::now(),
            },
        );
        broadcast_cursors(shared);
    }
    if let Some(message) = body["help"].as_str() {
        *shared.help.lock().unwrap() = Some(format!("{name}: {message}"));
        broadcast(
            shared,
            json!({"type": "help", "message": format!("{name}: {message}")}),
        );
        let _ = std::process::Command::new("notify-send")
            .args(["-u", "critical", "Agent needs you", message])
            .status();
    }
    json!({"ok": true})
}

/// A message to one viewer.
fn send_to(shared: &Shared, id: u64, message: Value) {
    if let Some(tx) = shared.outboxes.lock().unwrap().get(&id) {
        let _ = tx.send(message.to_string());
    }
}

fn broadcast(shared: &Shared, message: Value) {
    let text = message.to_string();
    shared
        .outboxes
        .lock()
        .unwrap()
        .retain(|_, tx| tx.send(text.clone()).is_ok());
}

fn broadcast_cursors(shared: &Shared) {
    let list: Vec<Participant> = shared
        .participants
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    broadcast(shared, json!({"type": "cursors", "participants": list}));
}

fn state() -> Value {
    state_for(None)
}

/// Hyprland state; `active_workspace` is the one shown on `output` (a phone
/// screen) when given, else Hyprland's focused workspace.
fn state_for(output: Option<&str>) -> Value {
    let clients = hypr::clients().unwrap_or_default();
    // Window positions are in layout coordinates; the viewer needs where its
    // monitor starts to draw outlines on the stream (a phone screen sits to
    // the right of the real one).
    let mon = output
        .and_then(monitor)
        .or_else(|| primary_output().ok().and_then(|(n, _, _)| monitor(&n)));
    // Logical size too: window sizes are logical, the stream is device pixels.
    let origin = mon
        .as_ref()
        .map(|m| {
            let scale = m["scale"].as_f64().unwrap_or(1.0).max(0.1);
            json!({"x": m["x"], "y": m["y"], "w": m["width"].as_f64().unwrap_or(0.0) / scale, "h": m["height"].as_f64().unwrap_or(0.0) / scale})
        })
        .unwrap_or(json!({"x": 0, "y": 0}));
    let active = output
        .and_then(monitor)
        .and_then(|m| m["activeWorkspace"]["id"].as_i64())
        .unwrap_or_else(|| hypr::active_workspace().unwrap_or(1));
    let focused = hypr::active_window();
    let mut counts: std::collections::BTreeMap<i64, usize> = (1..=9).map(|i| (i, 0)).collect();
    for c in &clients {
        if c.workspace.id > 0 {
            *counts.entry(c.workspace.id).or_default() += 1;
        }
    }
    json!({
        "type": "state",
        "origin": origin,
        // Agent spaces on this machine: which workspace is whose, what it's
        // doing, whether it's waiting for you.
        "agents": std::env::var_os("HOME").map(|h| crate::agent::list(std::path::Path::new(&h))).unwrap_or_default().iter().map(|a| json!({
            "agent": a.agent, "workspace": a.workspace, "task": a.task, "status": a.status, "help": a.help,
            "browsers": a.browsers.iter().map(|b| json!({"id": b.id, "pid": b.pid})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "active_workspace": active,
        "focused": focused,
        "workspaces": counts.iter().map(|(id, n)| json!({"id": id, "windows": n})).collect::<Vec<_>>(),
        "windows": clients.iter().filter(|c| c.workspace.id > 0).map(|c| json!({
            "address": c.address, "class": c.class, "title": c.title, "workspace": c.workspace.id,
            "floating": c.floating, "fullscreen": c.fullscreen, "at": c.at, "size": c.size,
            "mapped": c.mapped, "hidden": c.hidden, "focus": c.focus_history,
        })).collect::<Vec<_>>(),
    })
}

/// Push fresh state to every viewer on each Hyprland event (no polling).
/// Hyprland socket2 events that change what viewers see.
fn is_layout_event(line: &str) -> bool {
    [
        "workspace",
        "openwindow",
        "closewindow",
        "movewindow",
        "activewindow",
        "changefloatingmode",
        "fullscreen",
        "windowtitle",
        "monitoradded",
        "monitorremoved",
        "swapwindow",
    ]
    .iter()
    .any(|e| line.starts_with(&format!("{e}>>")) || line.starts_with(&format!("{e}v2>>")))
}

fn spawn_hyprland_events(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        loop {
            let path = format!(
                "{}/hypr/{}/.socket2.sock",
                std::env::var("XDG_RUNTIME_DIR").unwrap_or_default(),
                std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default()
            );
            if let Ok(sock) = std::os::unix::net::UnixStream::connect(&path) {
                // Short reads, so a due broadcast goes out on time.
                let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(10)));
                let mut reader = BufReader::new(sock);
                // Broadcast 40ms after the first layout event of a burst. Not
                // "after 40ms of quiet": while a screen is being captured,
                // Hyprland sends a `screencast` event every frame, so the
                // socket is never quiet and nothing would ever go out.
                let mut due: Option<std::time::Instant> = None;
                let mut line = String::new();
                loop {
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        // A timeout can land mid-line: read_line keeps what it
                        // read in `line` and the next call appends the rest, so
                        // only a line ending in '\n' is a whole event.
                        Ok(_) if line.ends_with('\n') => {
                            if is_layout_event(&line) && due.is_none() {
                                due = Some(
                                    std::time::Instant::now()
                                        + std::time::Duration::from_millis(40),
                                );
                            }
                            line.clear();
                        }
                        Ok(_) => {}
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) => {}
                        Err(e) => {
                            eprintln!("view: hyprland events: {e}");
                            break;
                        }
                    }
                    if due.is_some_and(|t| std::time::Instant::now() >= t) {
                        due = None;
                        broadcast(&shared, json!({"type": "state_changed"}));
                    }
                }
            } else {
                eprintln!("view: can't open {path}");
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

fn query(path: &str, key: &str) -> Option<String> {
    path.split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(String::from))
}

/// Choose the stream size for a viewer: the viewer's own pixel size capped at
/// the monitor, even dimensions (H.264 needs them).
fn source_for(shared: &Shared, path: &str, window: Option<&Value>) -> Source {
    let (output, mw, mh) = shared.output.clone();
    let want_w: u32 = query(path, "w")
        .and_then(|w| w.parse().ok())
        .unwrap_or(mw)
        .clamp(320, mw);
    let region = window.map(|w| {
        let at = &w["at"];
        let size = &w["size"];
        format!("{}x{}+{}+{}", size[0], size[1], at[0], at[1])
    });
    let (src_w, src_h) = window.map_or((mw, mh), |w| {
        (
            w["size"][0].as_u64().unwrap_or(1) as u32,
            w["size"][1].as_u64().unwrap_or(1) as u32,
        )
    });
    let scale = (f64::from(want_w) / f64::from(src_w)).min(1.0);
    let even = |v: f64| ((v as u32) / 2 * 2).max(2);
    let (w, h) = (
        even(f64::from(src_w) * scale),
        even(f64::from(src_h) * scale),
    );
    // 60 fps: the encoder's pipeline is ~1.5 frames deep, so this halves
    // click-to-photon (~92 → ~42 ms measured); damage-driven capture keeps an
    // idle screen free regardless of the rate.
    Source {
        output,
        region,
        width: w,
        height: h,
        fps: 60,
        virtual_output: false,
    }
}

/// A send or flush on the viewer's non-blocking socket: `Ok(true)` when it's
/// out, `Ok(false)` when the socket is full (the message stays queued and goes
/// out on a later flush), an error only when the connection is really gone.
fn queued(r: Result<(), tungstenite::Error>) -> anyhow::Result<bool> {
    match r {
        Ok(()) => Ok(true),
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(tungstenite::Error::WriteBufferFull(_)) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn viewer(
    mut ws: WebSocket<UnixStream>,
    shared: Arc<Shared>,
    who: Identity,
    path: &str,
) -> anyhow::Result<()> {
    let id = {
        let mut n = shared.next_id.lock().unwrap();
        *n += 1;
        *n
    };
    let name = query(path, "name")
        .map(|n| n.replace("%20", " "))
        .unwrap_or_else(|| who.name.clone());
    shared.participants.lock().unwrap().insert(
        id,
        Participant {
            id,
            name: name.clone(),
            kind: "human".into(),
            x: -1.0,
            y: -1.0,
            seen: std::time::Instant::now(),
        },
    );
    let (out_tx, out_rx) = mpsc::channel::<String>();
    shared.outboxes.lock().unwrap().insert(id, out_tx);

    // Window-only view captures the window's rectangle on screen, so bring
    // its workspace into view and focus it first; then read its live geometry.
    let window = query(path, "window").and_then(|addr| {
        let found = state()["windows"]
            .as_array()?
            .iter()
            .find(|w| w["address"] == addr.as_str())
            .cloned()?;
        lua_dispatch(&format!(
            "hl.dsp.focus({{ window = {} }})",
            hypr::lua_string(&format!("address:{addr}"))
        ));
        std::thread::sleep(std::time::Duration::from_millis(250));
        state()["windows"]
            .as_array()?
            .iter()
            .find(|w| w["address"] == addr.as_str())
            .cloned()
            .or(Some(found))
    });
    let phone = query(path, "phone").and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
    });
    let source = match phone {
        Some((w, h)) => {
            let scale = query(path, "scale")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(3.0);
            let workspace = query(path, "workspace")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| hypr::active_workspace().unwrap_or(1));
            phone_screen(&shared, id, w, h, scale, workspace)?
        }
        None => source_for(&shared, path, window.as_ref()),
    };
    let capture = shared.captures.get(&source)?;
    let frames = capture.subscribe();
    ws.send(Message::text(json!({"type": "hello", "id": id, "me": name, "stream": {"width": source.width, "height": source.height, "window": window.as_ref().map(|w| &w["address"])}, "output": {"width": source.width, "height": source.height}, "phone": phone.is_some()}).to_string()))?;
    let my_output = || {
        shared
            .phones
            .lock()
            .unwrap()
            .get(&id)
            .map(|(o, _)| o.clone())
    };
    ws.send(Message::text(state_for(my_output().as_deref()).to_string()))?;
    if let Some(help) = shared.help.lock().unwrap().clone() {
        ws.send(Message::text(
            json!({"type": "help", "message": help}).to_string(),
        ))?;
    }
    if let Some(by) = shared.taken_over.lock().unwrap().clone() {
        ws.send(Message::text(
            json!({"type": "control", "taken_over_by": by}).to_string(),
        ))?;
    }
    broadcast_cursors(&shared);

    ws.get_mut().set_nonblocking(true)?;
    let mut waiting_for_key = false;
    let result = (|| -> anyhow::Result<()> {
        loop {
            let mut idle = true;
            // Video: send what's queued; if more than ~1/3 s is waiting the
            // viewer is behind, so skip to the next keyframe.
            let mut pending: Vec<_> = frames.try_iter().collect();
            if pending.len() > 20 {
                if let Some(k) = pending.iter().rposition(|f| f.key) {
                    pending.drain(..k);
                } else {
                    pending.clear();
                    waiting_for_key = true;
                }
            }
            for frame in pending {
                if waiting_for_key && !frame.key {
                    continue;
                }
                // Skip unchanged-screen frames: nothing new to show.
                if crate::stream::is_static(&frame) {
                    continue;
                }
                waiting_for_key = false;
                let mut msg = Vec::with_capacity(frame.data.len() + 10);
                msg.push(1);
                msg.push(u8::from(frame.key));
                msg.extend(frame.pts_us.to_be_bytes());
                msg.extend_from_slice(&frame.data);
                // A slow phone link fills the socket: the frame stays queued
                // in tungstenite and goes out on the next flush. Dropping the
                // viewer here would cut it off mid-tap.
                if !queued(ws.send(Message::binary(msg)))? {
                    waiting_for_key = true;
                    break;
                }
                idle = false;
            }
            for text in out_rx.try_iter() {
                // "state_changed" becomes this viewer's own view of the state
                // (a phone screen shows a different workspace than the desk).
                let text = if text.contains("\"state_changed\"") {
                    state_for(my_output().as_deref()).to_string()
                } else {
                    text
                };
                queued(ws.send(Message::text(text)))?;
                idle = false;
            }
            // Anything still buffered from a full socket.
            queued(ws.flush())?;
            match ws.read() {
                Ok(Message::Text(t)) => {
                    idle = false;
                    if let Ok(msg) = serde_json::from_str::<Value>(&t) {
                        on_message(&shared, id, &name, &msg);
                    }
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            if idle {
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
        }
    })();
    shared.participants.lock().unwrap().remove(&id);
    shared.outboxes.lock().unwrap().remove(&id);
    // Stop this viewer's frames first: a recorder still running when its
    // virtual screen is removed segfaults (wf-recorder in gbm_bo_get_fd).
    drop(frames);
    if source.virtual_output {
        capture.stop();
    }
    shared.captures.stop_idle();
    end_phone_screen(&shared, id);
    // A take-over ends with the person who took over: the agent must not stay
    // paused for a closed tab. (Takers are tracked by viewer id, not name.)
    let taker = *shared.taken_by.lock().unwrap();
    if taker == Some(id) {
        set_control(&shared, None);
    }
    broadcast_cursors(&shared);
    result
}

/// Send input for viewer `id`: onto its phone screen if it has one, else the
/// primary monitor.
fn send_input_for(shared: &Shared, id: u64, event: Event) {
    let target = shared
        .phones
        .lock()
        .unwrap()
        .get(&id)
        .map(|(o, _)| o.clone())
        .unwrap_or_default();
    let mut inputs = shared.input.lock().unwrap();
    if !inputs.contains_key(&target) {
        // Connecting the virtual devices is a few Wayland round trips. Do it
        // on its own thread with a deadline: if the compositor is stuck, this
        // viewer's input is dropped instead of every viewer blocking on it.
        let (w, h, out) = if target.is_empty() {
            (shared.output.1, shared.output.2, None)
        } else {
            // Absolute coordinates in a fine grid; the compositor maps them
            // onto the bound output.
            (10_000, 10_000, Some(target.clone()))
        };
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(input::start(w, h, out.as_deref()));
        });
        let started = done_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the compositor didn't answer within 3s")));
        match started {
            Ok(tx) => {
                inputs.insert(target.clone(), tx);
            }
            Err(e) => {
                eprintln!("view: input for {target:?}: {e}");
                return;
            }
        }
    }
    if inputs
        .get(&target)
        .is_some_and(|tx| tx.send(event).is_err())
    {
        inputs.remove(&target);
    }
}

// ---- phone mode: a virtual Hyprland screen shaped like the phone ----------

fn monitor(name: &str) -> Option<Value> {
    let list = hypr::monitors_all().ok()?;
    list.as_array()?.iter().find(|m| m["name"] == name).cloned()
}

/// Create (or resize) a headless output at the phone's device pixels and
/// scale, move `workspace` onto it, and return the stream source for it.
/// Windows then tile for a tall narrow screen and apps get mobile widths.
fn phone_screen(
    shared: &Shared,
    id: u64,
    w: u32,
    h: u32,
    scale: f64,
    workspace: i64,
) -> anyhow::Result<Source> {
    // Unique across restarts: viewer ids start again at 1, and a name still
    // held by an old (disabled) output can't be created again.
    let name = format!("OSP-PHONE-{}-{id}", std::process::id());
    // Hyprland only accepts a scale that divides the mode into whole logical
    // pixels; round the size down to a multiple of the scale's denominator.
    let scale = clean_scale(scale);
    let step = scale_step(scale);
    let fit = |v: u32| (v.clamp(320, 3000) / step * step).max(step);
    let (w, h) = (fit(w), fit(h));
    // Register the rule before the output exists, so Hyprland never applies
    // Omarchy's catch-all `output = ""` rule (scale auto) to it first, which
    // raises an "invalid scale" error banner on a phone-shaped size.
    lua_dispatch_eval(&format!(
        "hl.monitor({{ output = {}, mode = {}, position = \"auto-right\", scale = {} }})",
        hypr::lua_string(&name),
        hypr::lua_string(&format!("{w}x{h}@60")),
        scale
    ));
    if monitor(&name).is_none() {
        let phones = hypr::monitors_all()?.as_array().map_or(0, |a| {
            // A disabled one is inert (Hyprland can't remove it until it
            // restarts): it shows nothing, so it doesn't count.
            a.iter()
                .filter(|m| is_phone_screen(&m["name"]) && m["disabled"] != true)
                .count()
        });
        anyhow::ensure!(
            phones < MAX_PHONE_SCREENS,
            "{phones} phone screens are already open on this machine; close a phone view first"
        );
        let out = hypr::ctl(
            &["output", "create", "headless", &name],
            std::time::Duration::from_secs(5),
        )?;
        anyhow::ensure!(out.contains("ok"), "could not create a virtual screen");
    }
    // Move the workspace: focus it on the primary monitor, then send it over.
    lua_dispatch(&format!(
        "hl.dsp.focus({{ monitor = {} }})",
        hypr::lua_string(&shared.output.0)
    ));
    lua_dispatch(&format!(
        "hl.dsp.focus({{ workspace = {} }})",
        hypr::lua_string(&workspace.to_string())
    ));
    lua_dispatch(&format!(
        "hl.dsp.workspace.move({{ monitor = {} }})",
        hypr::lua_string(&name)
    ));
    // Keyboard input follows Hyprland's focus: focus the phone's screen.
    lua_dispatch(&format!(
        "hl.dsp.focus({{ monitor = {} }})",
        hypr::lua_string(&name)
    ));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let m = monitor(&name).ok_or_else(|| anyhow::anyhow!("virtual screen vanished"))?;
    shared
        .phones
        .lock()
        .unwrap()
        .insert(id, (name.clone(), workspace));
    Ok(Source {
        output: name,
        region: None,
        width: m["width"].as_u64().unwrap_or(u64::from(w)) as u32,
        height: m["height"].as_u64().unwrap_or(u64::from(h)) as u32,
        fps: 60,
        virtual_output: true,
    })
}

/// Scales Hyprland renders cleanly (whole and common fractional ones).
fn clean_scale(s: f64) -> f64 {
    const OK: [f64; 6] = [1.0, 1.25, 1.5, 2.0, 2.5, 3.0];
    OK.into_iter()
        .min_by(|a, b| (a - s).abs().total_cmp(&(b - s).abs()))
        .unwrap_or(2.0)
}

/// The size multiple that makes `size / scale` a whole number: for scale p/q
/// in lowest terms, sizes must be multiples of p (3 → 3, 2.5 → 5, 1.25 → 5).
fn scale_step(s: f64) -> u32 {
    let q = (1..=8)
        .find(|d| (s * f64::from(*d)).fract() == 0.0)
        .unwrap_or(4);
    (s * f64::from(q)).round() as u32
}

/// At most this many phone screens at once (one per phone watching): each is a
/// real Hyprland output, and a runaway client must not be able to pile them up.
const MAX_PHONE_SCREENS: usize = 4;

fn is_phone_screen(name: &Value) -> bool {
    name.as_str().is_some_and(|n| n.starts_with("OSP-PHONE-"))
}

/// Remove one phone screen without ever stalling the compositor's caller:
/// its workspace goes back to `primary` first, then the output is removed,
/// each step with a deadline. (Don't disable it first: Hyprland can't remove
/// a disabled headless output, and it would linger.) Returns false if Hyprland
/// stopped answering, so the caller can stop issuing more removals.
fn remove_phone_screen(name: &str, workspace: Option<i64>, primary: &str, disabled: bool) -> bool {
    if let Some(ws) = workspace.filter(|w| *w > 0) {
        lua_dispatch(&format!(
            "hl.dsp.focus({{ workspace = {} }})",
            hypr::lua_string(&ws.to_string())
        ));
        lua_dispatch(&format!(
            "hl.dsp.workspace.move({{ monitor = {} }})",
            hypr::lua_string(primary)
        ));
    }
    if disabled {
        // Hyprland can't remove a disabled headless output (it answers
        // "output not found"); it goes when Hyprland restarts. It shows
        // nothing and doesn't count toward the limit, so leave it alone.
        return true;
    }
    match hypr::ctl(
        &["output", "remove", name],
        std::time::Duration::from_secs(5),
    ) {
        Ok(out) if out.contains("ok") => true,
        Ok(out) => {
            eprintln!("view: removing {name}: {}", out.trim());
            true
        }
        Err(e) => {
            eprintln!("view: removing {name}: {e}");
            false
        }
    }
}

/// Phone screens left by an earlier run (the view was restarted while a phone
/// was watching) keep a workspace off the real monitor. Remove them one at a
/// time on a background thread, so startup never waits on the compositor,
/// and stop at the first one Hyprland doesn't answer for.
fn remove_stale_phone_screens(primary: &str) {
    let Ok(list) = hypr::monitors_all() else {
        return;
    };
    let stale: Vec<(String, Option<i64>, bool)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| is_phone_screen(&m["name"]))
        .map(|m| {
            (
                m["name"].as_str().unwrap_or("").to_string(),
                m["activeWorkspace"]["id"].as_i64(),
                m["disabled"].as_bool().unwrap_or(false),
            )
        })
        .collect();
    if stale.is_empty() {
        return;
    }
    let primary = primary.to_string();
    std::thread::spawn(move || {
        for (name, ws, disabled) in stale {
            if disabled {
                // Inert until Hyprland restarts; it can't be removed.
                continue;
            }
            if !remove_phone_screen(&name, ws, &primary, false) {
                eprintln!("view: Hyprland isn't answering; left the other phone screens in place");
                return;
            }
            eprintln!("view: removed leftover phone screen {name}");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
}

fn end_phone_screen(shared: &Shared, id: u64) {
    let Some((name, workspace)) = shared.phones.lock().unwrap().remove(&id) else {
        return;
    };
    shared.input.lock().unwrap().remove(&name);
    remove_phone_screen(&name, Some(workspace), &shared.output.0, false);
}

/// In phone mode, bring workspace `target` onto this viewer's phone screen:
/// the workspace it was showing goes back to the real monitor, `target` moves
/// over (wherever it was). Returns the phone output's name.
fn phone_swap(shared: &Shared, id: u64, target: i64) -> Option<String> {
    let (name, current) = shared.phones.lock().unwrap().get(&id).cloned()?;
    if current == target {
        return Some(name);
    }
    let ws = |n: i64| hypr::lua_string(&n.to_string());
    // Old workspace back to the monitor it belongs on.
    lua_dispatch(&format!("hl.dsp.focus({{ workspace = {} }})", ws(current)));
    lua_dispatch(&format!(
        "hl.dsp.workspace.move({{ monitor = {} }})",
        hypr::lua_string(&shared.output.0)
    ));
    // Target workspace onto the phone screen (it may not exist yet: focusing
    // creates it on the focused monitor, then it moves).
    lua_dispatch(&format!(
        "hl.dsp.focus({{ monitor = {} }})",
        hypr::lua_string(&shared.output.0)
    ));
    lua_dispatch(&format!("hl.dsp.focus({{ workspace = {} }})", ws(target)));
    lua_dispatch(&format!(
        "hl.dsp.workspace.move({{ monitor = {} }})",
        hypr::lua_string(&name)
    ));
    lua_dispatch(&format!(
        "hl.dsp.focus({{ monitor = {} }})",
        hypr::lua_string(&name)
    ));
    shared
        .phones
        .lock()
        .unwrap()
        .insert(id, (name.clone(), target));
    Some(name)
}

fn lua_dispatch_eval(lua: &str) {
    if let Err(e) = hypr::eval(lua) {
        eprintln!("view: {e}");
    }
}

fn lua_dispatch(lua: &str) {
    if let Err(e) = hypr::dispatch(lua) {
        eprintln!("view: {e}");
    }
}

/// Take over (`Some((name, viewer id))`) or hand back (`None`): pauses or
/// resumes the agents reporting here and, through agents.json, every agent's
/// MCP tools.
fn set_control(shared: &Shared, by: Option<(&str, u64)>) {
    *shared.taken_over.lock().unwrap() = by.map(|(n, _)| n.to_string());
    *shared.taken_by.lock().unwrap() = by.map(|(_, id)| id);
    if by.is_some() {
        *shared.help.lock().unwrap() = None;
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    if let Err(e) = crate::agent::set_taken_over(&home, by.map(|(n, _)| n)) {
        eprintln!("view: recording the take-over for agents: {e}");
    }
    broadcast(
        shared,
        json!({"type": "control", "taken_over_by": by.map(|(n, _)| n)}),
    );
}

fn on_message(shared: &Shared, id: u64, name: &str, msg: &Value) {
    let addr =
        |m: &Value| hypr::lua_string(&format!("address:{}", m["address"].as_str().unwrap_or("")));
    match msg["type"].as_str().unwrap_or("") {
        "pointer" => {
            let (x, y) = (
                msg["x"].as_f64().unwrap_or(0.0),
                msg["y"].as_f64().unwrap_or(0.0),
            );
            if let Some(p) = shared.participants.lock().unwrap().get_mut(&id) {
                p.x = x;
                p.y = y;
            }
            send_input_for(shared, id, Event::Move { x, y });
            if let Some(b) = msg["button"].as_u64() {
                send_input_for(
                    shared,
                    id,
                    Event::Button {
                        button: input::button(b as u32),
                        pressed: msg["down"].as_bool().unwrap_or(false),
                    },
                );
            }
            broadcast_cursors(shared);
        }
        "scroll" => send_input_for(
            shared,
            id,
            Event::Scroll {
                dx: msg["dx"].as_f64().unwrap_or(0.0),
                dy: msg["dy"].as_f64().unwrap_or(0.0),
            },
        ),
        "key" => {
            // Real key events: modifier keys arrive as their own key events and
            // are tracked by the input thread; sticky buttons add `sticky`.
            if let Some(code) = msg["code"].as_str().and_then(input::evdev) {
                let pressed = msg["down"].as_bool().unwrap_or(true);
                send_input_for(
                    shared,
                    id,
                    Event::Mods {
                        mask: msg["sticky"].as_u64().unwrap_or(0) as u32,
                    },
                );
                send_input_for(shared, id, Event::Key { code, pressed });
                // Sticky modifiers last one key: left set, Hyprland (which
                // merges modifiers across keyboards) would see SUPER held for
                // every later key from any device.
                if !pressed {
                    send_input_for(shared, id, Event::Mods { mask: 0 });
                }
            }
        }
        "combo" => {
            // e.g. {mods: 64, code: "Digit3"}: press and release with mods held.
            if let Some(code) = msg["code"].as_str().and_then(input::evdev) {
                send_input_for(
                    shared,
                    id,
                    Event::Mods {
                        mask: msg["mods"].as_u64().unwrap_or(0) as u32,
                    },
                );
                send_input_for(
                    shared,
                    id,
                    Event::Key {
                        code,
                        pressed: true,
                    },
                );
                send_input_for(
                    shared,
                    id,
                    Event::Key {
                        code,
                        pressed: false,
                    },
                );
                send_input_for(shared, id, Event::Mods { mask: 0 });
            }
        }
        "focus_workspace" => {
            let target = msg["workspace"].as_i64().unwrap_or(1);
            if let Some(name) = phone_swap(shared, id, target) {
                // The phone's screen now shows `target`; refresh everyone.
                std::thread::sleep(std::time::Duration::from_millis(150));
                let _ = name;
                broadcast(shared, json!({"type": "state_changed"}));
            } else {
                lua_dispatch(&format!(
                    "hl.dsp.focus({{ workspace = {} }})",
                    hypr::lua_string(&target.to_string())
                ));
            }
        }
        "focus_window" => lua_dispatch(&format!("hl.dsp.focus({{ window = {} }})", addr(msg))),
        "move_window" => lua_dispatch(&format!(
            "hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})",
            hypr::lua_string(&msg["workspace"].to_string()),
            addr(msg)
        )),
        "close_window" => lua_dispatch(&format!(
            "hl.dsp.window.close({{ window = {} }})",
            addr(msg)
        )),
        // Long-press a tile, drop it on another: swap them (Omarchy's
        // SUPER+SHIFT+arrow, but with a chosen partner).
        "swap_windows" => {
            let other =
                hypr::lua_string(&format!("address:{}", msg["with"].as_str().unwrap_or("")));
            lua_dispatch(&format!(
                "hl.dsp.window.swap({{ window = {}, target = {} }})",
                addr(msg),
                other
            ));
        }
        // Hand one of your browser windows to an agent's space (with your
        // sign-ins), or take an agent's browser back.
        "hand_to_agent" => {
            let (window, who) = (
                msg["address"].as_str().unwrap_or("").to_string(),
                msg["agent"].as_str().unwrap_or("").to_string(),
            );
            let shared_home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            let reply = match crate::agent::hand_over(&shared_home, &who, &window) {
                Ok(b) => {
                    json!({"type": "toast", "text": format!("✓ handed to {who} (workspace {})", crate::agent::space_of(&shared_home, &who).map(|s| s.workspace).unwrap_or(0)), "browser": b.id})
                }
                Err(e) => json!({"type": "toast", "text": format!("✕ {e}")}),
            };
            send_to(shared, id, reply);
            broadcast(shared, json!({"type": "state_changed"}));
        }
        "take_back_browser" => {
            let who = msg["agent"].as_str().unwrap_or("").to_string();
            let shared_home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            let reply = match crate::agent::take_back(&shared_home, &who, msg["browser"].as_str()) {
                Ok(tabs) => {
                    json!({"type": "toast", "text": format!("✓ took back {} tab(s) from {who}", tabs.len())})
                }
                Err(e) => json!({"type": "toast", "text": format!("✕ {e}")}),
            };
            send_to(shared, id, reply);
            broadcast(shared, json!({"type": "state_changed"}));
        }
        "help_done" => {
            let who = msg["agent"].as_str().unwrap_or("");
            let shared_home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            let _ = crate::agent::update(&shared_home, who, Some("back to work"), Some(None));
            broadcast(shared, json!({"type": "state_changed"}));
        }
        "toggle_floating" => lua_dispatch(&format!(
            "hl.dsp.window.float({{ window = {} }})",
            addr(msg)
        )),
        "fullscreen" => lua_dispatch(&format!(
            "hl.dsp.window.fullscreen({{ window = {} }})",
            addr(msg)
        )),
        "omarchy" => {
            let cmd = match msg["what"].as_str().unwrap_or("") {
                "menu" => "omarchy-menu toggle",
                "launcher" => "omarchy-menu toggle apps",
                "terminal" => "omarchy-launch-terminal",
                "browser" => "omarchy-launch-browser",
                _ => return,
            };
            lua_dispatch(&format!("hl.dsp.exec_cmd({})", hypr::lua_string(cmd)));
        }
        "take_over" => set_control(shared, Some((name, id))),
        "hand_back" => set_control(shared, None),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_partly_read_event_line_is_not_lost() {
        // What a 40ms read timeout in the middle of "openwindow>>…" leaves:
        // read_line keeps "openwi", then appends the rest on the next read.
        use std::io::BufRead;
        let mut r = std::io::BufReader::new(&b"openwi"[..]);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(
            !line.ends_with('\n'),
            "a partial line must wait for the rest"
        );
        let mut r = std::io::BufReader::new(&b"ndow>>5d4b,9,foot,foot\n"[..]);
        r.read_line(&mut line).unwrap();
        assert!(line.ends_with('\n') && super::is_layout_event(&line));
        assert!(!super::is_layout_event("bell>>x\n"));
    }

    #[test]
    fn omarchy_theme_colors_become_css_variables() {
        let css = super::theme_css(
            "mode = \"dark\"\naccent = \"#7aa2f7\"\ndark_background = \"#13141c\"\nbad = \"red; }\"\n",
        );
        assert_eq!(
            css,
            ":root{--t-accent:#7aa2f7;--t-dark-background:#13141c;}"
        );
        assert_eq!(super::theme_css(""), "");
    }

    #[test]
    fn phone_screens_use_scales_hyprland_accepts() {
        assert_eq!(super::clean_scale(2.6), 2.5);
        assert_eq!(super::clean_scale(3.0), 3.0);
        assert_eq!(super::scale_step(2.5), 5);
        assert_eq!(super::scale_step(1.25), 5);
        assert_eq!(super::scale_step(3.0), 3);
        assert_eq!(super::scale_step(2.0), 2);
    }

    #[test]
    fn query_values_are_read_from_the_url() {
        assert_eq!(
            super::query("/ws?w=390&name=Cole", "w").as_deref(),
            Some("390")
        );
        assert_eq!(
            super::query("/ws?w=390&name=Cole", "name").as_deref(),
            Some("Cole")
        );
        assert_eq!(super::query("/ws", "w"), None);
    }

    fn headers(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const OWN: &str = "https://alpha.tail1234.ts.net:7788";

    #[test]
    fn the_origin_is_this_views_own_https_host() {
        assert_eq!(super::view_origin("Alpha.tail1234.ts.net."), OWN);
    }

    #[test]
    fn other_sites_cannot_open_the_view() {
        assert!(super::origin_allowed(&headers(&[("origin", OWN)]), OWN));
        assert!(
            super::origin_allowed(&headers(&[]), OWN),
            "no Origin: not a web page"
        );
        assert!(!super::origin_allowed(
            &headers(&[("origin", "https://evil.example")]),
            OWN
        ));
        assert!(
            !super::origin_allowed(&headers(&[("origin", "null")]), OWN),
            "sandboxed iframes send null"
        );
        assert!(
            !super::origin_allowed(
                &headers(&[("origin", "https://bravo.tail1234.ts.net:7788")]),
                OWN
            ),
            "another machine's page is not this one"
        );
        assert!(
            !super::origin_allowed(
                &headers(&[("origin", "https://alpha.tail1234.ts.net:10001")]),
                OWN
            ),
            "another service on this host"
        );
    }

    #[test]
    fn presence_is_readable_only_by_view_pages_on_this_tailnet() {
        assert!(super::presence_origin_allowed(
            "https://bravo.tail1234.ts.net:7788",
            OWN
        ));
        assert!(super::presence_origin_allowed(OWN, OWN));
        assert!(!super::presence_origin_allowed("https://evil.example", OWN));
        assert!(
            !super::presence_origin_allowed("https://bravo.tail1234.ts.net:10001", OWN),
            "another service"
        );
        assert!(!super::presence_origin_allowed(
            "https://a.b.tail1234.ts.net:7788",
            OWN
        ));
        assert!(!super::presence_origin_allowed(
            "https://tail1234.ts.net.evil.example:7788",
            OWN
        ));
        assert!(!super::presence_origin_allowed(
            "http://bravo.tail1234.ts.net:7788",
            OWN
        ));
    }

    #[test]
    fn the_caller_is_the_address_serve_forwards() {
        assert_eq!(
            super::forwarded_ip(&headers(&[("x-forwarded-for", "100.64.0.10")])).unwrap(),
            "100.64.0.10"
        );
        assert_eq!(
            super::forwarded_ip(&headers(&[("x-forwarded-for", "100.64.0.1, 100.64.0.10")]))
                .unwrap(),
            "100.64.0.10",
            "a client-sent value in front of Serve's is ignored"
        );
        assert!(
            super::forwarded_ip(&headers(&[("x-forwarded-for", "100.64.0.10, 8.8.8.8")])).is_err()
        );
        assert!(super::forwarded_ip(&headers(&[])).is_err());
    }
}
