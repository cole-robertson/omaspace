//! A small Chrome DevTools Protocol client: what an agent needs to use a
//! browser without the mouse or keyboard (so it never takes your focus).
//! One connection per call keeps it simple; agents act at human speed.
//!
//! Agent browsers hold a copy of your sign-ins, so their DevTools is never on
//! a TCP port (any user on the machine could connect to loopback and read
//! every cookie). Chromium runs with `--remote-debugging-pipe` under
//! `omaspace cdp-broker`, which relays it on a unix socket only you can open.
//! Messages on the pipe and the socket are JSON, each ended by a NUL byte.

use anyhow::Context;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// A browser's DevTools, through its broker's socket.
#[derive(Debug, Clone)]
pub struct Browser {
    pub socket: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// Where the broker for agent browser `id` listens.
pub fn socket_for(id: &str) -> PathBuf {
    crate::sock::runtime_dir()
        .join("omaspace-cdp")
        .join(format!("{id}.sock"))
}

/// One DevTools connection: browser-level, or attached to a tab.
struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Conn {
    fn open(socket: &Path) -> anyhow::Result<Conn> {
        let stream = UnixStream::connect(socket)
            .with_context(|| format!("connecting to {}", socket.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        Ok(Conn {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next: 0,
        })
    }

    /// One command; returns its `result` (events in between are skipped).
    fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let mut bytes = msg.to_string().into_bytes();
        bytes.push(0);
        self.writer.write_all(&bytes)?;
        loop {
            let mut raw = Vec::new();
            anyhow::ensure!(
                self.reader.read_until(0, &mut raw)? > 0,
                "the browser closed DevTools"
            );
            raw.pop_if(|b| *b == 0);
            let v: Value = serde_json::from_slice(&raw)?;
            if v["id"] == id {
                if let Some(e) = v.get("error") {
                    anyhow::bail!("{method}: {}", e["message"].as_str().unwrap_or("failed"));
                }
                return Ok(v["result"].clone());
            }
        }
    }
}

impl Browser {
    fn pages(&self, conn: &mut Conn) -> anyhow::Result<Vec<Value>> {
        let r = conn.call(None, "Target.getTargets", json!({}))?;
        Ok(r["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "page")
            .cloned()
            .collect())
    }

    /// Whether the browser answers.
    pub fn alive(&self) -> bool {
        Conn::open(&self.socket)
            .and_then(|mut c| c.call(None, "Browser.getVersion", json!({})))
            .is_ok()
    }

    /// Open pages (not extensions or browser UI), in the browser's order.
    pub fn tabs(&self) -> anyhow::Result<Vec<Tab>> {
        let mut conn = Conn::open(&self.socket)?;
        Ok(self
            .pages(&mut conn)?
            .iter()
            .map(|t| Tab {
                id: t["targetId"].as_str().unwrap_or("").into(),
                url: t["url"].as_str().unwrap_or("").into(),
                title: t["title"].as_str().unwrap_or("").into(),
            })
            .collect())
    }

    /// Ask the browser to quit cleanly (it flushes its cookie store).
    pub fn close(&self) -> anyhow::Result<()> {
        Conn::open(&self.socket)?.call(None, "Browser.close", json!({}))?;
        Ok(())
    }

    /// A session on one tab: `tab` by id, or else the tab the window is
    /// showing (the HTTP `/json/list` put that first; `Target.getTargets` is
    /// in no useful order, so ask each page whether it is visible).
    pub fn page(&self, tab: Option<&str>) -> anyhow::Result<Page> {
        let mut conn = Conn::open(&self.socket)?;
        let pages = self.pages(&mut conn)?;
        let ids: Vec<&str> = pages
            .iter()
            .filter_map(|t| t["targetId"].as_str())
            .collect();
        let id = match tab {
            Some(id) => *ids.iter().find(|t| **t == id).context("no such tab")?,
            None => {
                let first = *ids.first().context("no such tab")?;
                let mut shown = None;
                for id in &ids {
                    let session = attach(&mut conn, id)?;
                    let visible = conn.call(
                        Some(&session),
                        "Runtime.evaluate",
                        json!({"expression": "document.visibilityState", "returnByValue": true}),
                    );
                    let _ = conn.call(
                        None,
                        "Target.detachFromTarget",
                        json!({"sessionId": session}),
                    );
                    if visible.is_ok_and(|v| v["result"]["value"] == "visible") {
                        shown = Some(*id);
                        break;
                    }
                }
                shown.unwrap_or(first)
            }
        };
        let session = attach(&mut conn, id)?;
        Ok(Page { conn, session })
    }
}

fn attach(conn: &mut Conn, target: &str) -> anyhow::Result<String> {
    let r = conn.call(
        None,
        "Target.attachToTarget",
        json!({"targetId": target, "flatten": true}),
    )?;
    Ok(r["sessionId"]
        .as_str()
        .context("could not attach to the tab")?
        .to_string())
}

pub struct Page {
    conn: Conn,
    session: String,
}

impl Drop for Page {
    fn drop(&mut self) {
        let session = self.session.clone();
        let _ = self.conn.call(
            None,
            "Target.detachFromTarget",
            json!({"sessionId": session}),
        );
    }
}

impl Page {
    /// One CDP command on this tab; returns its `result`.
    pub fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let session = self.session.clone();
        self.conn.call(Some(&session), method, params)
    }

    /// Evaluate JavaScript in the page; returns its value.
    pub fn eval(&mut self, expression: &str) -> anyhow::Result<Value> {
        let r = self.call(
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        )?;
        if let Some(ex) = r.get("exceptionDetails") {
            anyhow::bail!(
                "page error: {}",
                ex["exception"]["description"]
                    .as_str()
                    .or(ex["text"].as_str())
                    .unwrap_or("failed")
            );
        }
        Ok(r["result"]["value"].clone())
    }

    /// Go to `url` and wait (up to 15s) for the page to finish loading.
    pub fn navigate(&mut self, url: &str) -> anyhow::Result<()> {
        self.call("Page.navigate", json!({"url": url}))?;
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(150));
            if self
                .eval("document.readyState")
                .ok()
                .as_ref()
                .and_then(Value::as_str)
                == Some("complete")
            {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Click the element matching a CSS selector (scrolled into view).
    pub fn click(&mut self, selector: &str) -> anyhow::Result<()> {
        let at = self.eval(&format!(
            "(() => {{ const e = document.querySelector({}); if (!e) return null; e.scrollIntoView({{block:'center'}}); const r = e.getBoundingClientRect(); return [r.x + r.width/2, r.y + r.height/2]; }})()",
            json!(selector)
        ))?;
        let (x, y) = (
            at[0].as_f64().context("no element matches that selector")?,
            at[1].as_f64().unwrap_or(0.0),
        );
        self.click_at(x, y)
    }

    pub fn click_at(&mut self, x: f64, y: f64) -> anyhow::Result<()> {
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            )?;
        }
        Ok(())
    }

    /// Type text into whatever has focus in the page (focus `selector` first
    /// if given).
    pub fn type_text(&mut self, selector: Option<&str>, text: &str) -> anyhow::Result<()> {
        if let Some(sel) = selector {
            let ok = self.eval(&format!("(() => {{ const e = document.querySelector({}); if (!e) return false; e.focus(); return true; }})()", json!(sel)))?;
            anyhow::ensure!(ok == json!(true), "no element matches {sel}");
        }
        self.call("Input.insertText", json!({"text": text}))?;
        Ok(())
    }

    /// Press a key (Enter, Tab, Escape, ArrowDown, Backspace…).
    pub fn key(&mut self, key: &str) -> anyhow::Result<()> {
        let (code, vk, text) = match key {
            "Enter" => ("Enter", 13, "\r"),
            "Tab" => ("Tab", 9, ""),
            "Escape" => ("Escape", 27, ""),
            "Backspace" => ("Backspace", 8, ""),
            "ArrowDown" => ("ArrowDown", 40, ""),
            "ArrowUp" => ("ArrowUp", 38, ""),
            "ArrowLeft" => ("ArrowLeft", 37, ""),
            "ArrowRight" => ("ArrowRight", 39, ""),
            other => anyhow::bail!("unsupported key {other}"),
        };
        for kind in ["keyDown", "keyUp"] {
            let mut p =
                json!({"type": kind, "key": key, "code": code, "windowsVirtualKeyCode": vk});
            if kind == "keyDown" && !text.is_empty() {
                p["text"] = json!(text);
            }
            self.call("Input.dispatchKeyEvent", p)?;
        }
        Ok(())
    }

    /// The page's URL, title and visible text (trimmed).
    pub fn read(&mut self, max_chars: usize) -> anyhow::Result<Value> {
        self.eval(&format!(
            "({{ url: location.href, title: document.title, text: (document.body ? document.body.innerText : '').slice(0, {max_chars}) }})"
        ))
    }

    /// PNG screenshot of the page, base64.
    pub fn screenshot(&mut self) -> anyhow::Result<String> {
        let r = self.call("Page.captureScreenshot", json!({"format": "png"}))?;
        Ok(r["data"].as_str().unwrap_or("").to_string())
    }

    /// Cookies visible to the page's origin (tests, sign-in checks).
    pub fn cookie_names(&mut self) -> anyhow::Result<Vec<String>> {
        let r = self.call("Network.getCookies", json!({}))?;
        Ok(r["cookies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["name"].as_str().map(String::from))
            .collect())
    }
}

// ---- the broker: `omaspace cdp-broker <socket> -- <browser argv...>` --------

/// The pipes between the relay and the browser. Chromium with
/// `--remote-debugging-pipe` reads commands on fd 3 and writes replies on fd 4.
struct Pipes {
    to_browser: std::io::PipeWriter,
    from_browser: std::io::PipeReader,
    /// The browser's ends, at fd >= 10 so moving them onto 3 and 4 can never
    /// be a no-op dup2 onto itself (which would leave close-on-exec set).
    cmd_fd: OwnedFd,
    reply_fd: OwnedFd,
}

impl Pipes {
    fn new() -> anyhow::Result<Pipes> {
        let (to_browser_r, to_browser) = std::io::pipe()?;
        let (from_browser, from_browser_w) = std::io::pipe()?;
        let cmd_fd = dup_high(to_browser_r.as_raw_fd())?;
        let reply_fd = dup_high(from_browser_w.as_raw_fd())?;
        Ok(Pipes {
            to_browser,
            from_browser,
            cmd_fd,
            reply_fd,
        })
    }

    /// `program args --remote-debugging-pipe` with the browser's ends on 3 and 4.
    fn command(&self, program: &str, args: &[String]) -> std::process::Command {
        let (c, r) = (self.cmd_fd.as_raw_fd(), self.reply_fd.as_raw_fd());
        let mut command = std::process::Command::new(program);
        command.args(args).arg("--remote-debugging-pipe");
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(&mut command, move || {
                if dup2(c, 3) < 0 || dup2(r, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
    }

    /// Keep only the relay's ends, so the browser closing its end is seen.
    fn relay_ends(self) -> (std::io::PipeWriter, std::io::PipeReader) {
        (self.to_browser, self.from_browser)
    }
}

/// Become the browser, its DevTools relayed on `socket` by a forked helper.
///
/// The browser keeps this process (it execs in place) because Hyprland's
/// per-launch rule (`exec_cmd(cmd, { workspace = "N silent" })`) binds to the
/// pid it started: a browser started as a child would open on your workspace.
/// Returns only on failure.
pub fn broker(socket: &Path, argv: &[String]) -> anyhow::Result<()> {
    let (program, args) = argv.split_first().context("no browser to run")?;
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    let listener = crate::sock::bind_private(socket)?;
    let pipes = Pipes::new()?;
    let mut command = pipes.command(program, args);
    // Still single-threaded here (nothing has spawned a thread), so the
    // forked helper is a complete copy and may start threads of its own.
    match unsafe { fork() } {
        -1 => Err(std::io::Error::last_os_error().into()),
        0 => {
            drop(command);
            let (to_browser, from_browser) = pipes.relay_ends();
            relay(listener, socket, to_browser, from_browser, |_| {
                std::process::exit(0)
            });
            std::process::exit(1)
        }
        _ => {
            // The socket and the relay's pipe ends are close-on-exec.
            let error = std::os::unix::process::CommandExt::exec(&mut command);
            Err(anyhow::anyhow!("starting {program}: {error}"))
        }
    }
}

/// Relay DevTools between `listener` and the browser, one client at a time.
/// When the browser closes its end (it exited), the socket is removed and
/// `on_exit` runs (in the helper it exits the process).
fn relay(
    listener: UnixListener,
    socket: &Path,
    to_browser: std::io::PipeWriter,
    from_browser: std::io::PipeReader,
    on_exit: fn(()),
) {
    let to_browser = Arc::new(Mutex::new(to_browser));
    let (events_tx, events_rx) = mpsc::channel::<Vec<u8>>();
    let cleanup = socket.to_path_buf();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(from_browser);
        loop {
            let mut msg = Vec::new();
            match reader.read_until(0, &mut msg) {
                Ok(n) if n > 0 => {
                    if events_tx.send(msg).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = std::fs::remove_file(&cleanup);
        on_exit(());
    });
    for client in listener.incoming().flatten() {
        // Replies meant for an earlier client are not this one's.
        while events_rx.try_recv().is_ok() {}
        let _ = serve_client(client, &to_browser, &events_rx);
    }
}

fn serve_client(
    client: UnixStream,
    to_browser: &Arc<Mutex<std::io::PipeWriter>>,
    from_browser: &mpsc::Receiver<Vec<u8>>,
) -> anyhow::Result<()> {
    // A client idle this long is dropped, so a stuck one can't hold the browser.
    client.set_read_timeout(Some(Duration::from_secs(120)))?;
    let done = Arc::new(AtomicBool::new(false));
    let (reader, flag, pipe) = (client.try_clone()?, done.clone(), to_browser.clone());
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        loop {
            let mut msg = Vec::new();
            match reader.read_until(0, &mut msg) {
                Ok(n) if n > 0 && msg.ends_with(&[0]) => {
                    if pipe.lock().unwrap().write_all(&msg).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        flag.store(true, Ordering::SeqCst);
    });
    let mut writer = client;
    while !done.load(Ordering::SeqCst) {
        match from_browser.recv_timeout(Duration::from_millis(50)) {
            Ok(msg) => writer.write_all(&msg)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = writer.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn dup_high(fd: i32) -> std::io::Result<OwnedFd> {
    const F_DUPFD_CLOEXEC: i32 = 1030;
    let new = unsafe { fcntl(fd, F_DUPFD_CLOEXEC, 10) };
    if new < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(new) })
}

unsafe extern "C" {
    fn dup2(old: i32, new: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
    fn fork() -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in browser: reads NUL-ended commands on fd 3 and answers each
    /// on fd 4, the way Chromium does with --remote-debugging-pipe.
    const FAKE: &str = r#"
        while IFS= read -r -d '' msg <&3; do
            id=$(printf '%s' "$msg" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
            case $msg in
                *Browser.close*) printf '{"id":%s,"result":{}}\0' "$id" >&4; exit 0 ;;
                *Target.attachToTarget*) printf '{"id":%s,"result":{"sessionId":"S1"}}\0' "$id" >&4 ;;
                *Target.getTargets*) printf '{"method":"Target.targetCreated"}\0{"id":%s,"result":{"targetInfos":[{"targetId":"T1","type":"page","url":"https://a.example/","title":"A"},{"targetId":"W","type":"service_worker"}]}}\0' "$id" >&4 ;;
                *visibilityState*) printf '{"id":%s,"result":{"result":{"value":"visible"}}}\0' "$id" >&4 ;;
                *) printf '{"id":%s,"result":{"ok":true}}\0' "$id" >&4 ;;
            esac
        done"#;

    /// What the helper does, in-process: the browser is spawned here instead
    /// of exec'd (the test harness can't become the browser).
    fn start(socket: &Path, argv: &[String], on_exit: fn(())) {
        let dir = socket.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let listener = crate::sock::bind_private(socket).unwrap();
        let pipes = Pipes::new().unwrap();
        let mut browser = pipes.command(&argv[0], &argv[1..]).spawn().unwrap();
        std::thread::spawn(move || browser.wait());
        let (to_browser, from_browser) = pipes.relay_ends();
        let at = socket.to_path_buf();
        std::thread::spawn(move || relay(listener, &at, to_browser, from_browser, on_exit));
    }

    #[test]
    fn the_broker_relays_devtools_on_a_private_socket_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("cdp/b.sock");
        let argv: Vec<String> = ["bash", "-c", FAKE, "fake-browser"]
            .map(String::from)
            .to_vec();
        static EXITED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        start(&socket, &argv, |()| {
            let _ = EXITED.set(());
        });
        let b = Browser {
            socket: socket.clone(),
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !b.alive() {
            assert!(
                std::time::Instant::now() < deadline,
                "broker never answered"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let tabs = b.tabs().unwrap();
        assert_eq!(
            tabs,
            vec![Tab {
                id: "T1".into(),
                url: "https://a.example/".into(),
                title: "A".into()
            }],
            "pages only; events skipped"
        );
        let mut page = b.page(None).unwrap();
        assert_eq!(
            page.call("Runtime.evaluate", json!({})).unwrap()["ok"],
            true
        );
        drop(page);
        b.close().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while EXITED.get().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the browser never exited"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!socket.exists(), "the socket goes with the browser");
    }

    /// The same against a real headless Chromium: `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs Chromium"]
    fn a_real_chromium_is_driven_through_the_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("cdp/c.sock");
        let argv: Vec<String> = vec![
            "/usr/lib/chromium/chromium".into(),
            "--headless=new".into(),
            format!("--user-data-dir={}", dir.path().join("profile").display()),
            "--no-first-run".into(),
            "data:text/html,<title>t</title><input id=q><p>pipe</p>".into(),
        ];
        start(&socket, &argv, |()| {});
        let b = Browser { socket };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while b.tabs().map_or(true, |t| t.is_empty()) {
            assert!(
                std::time::Instant::now() < deadline,
                "chromium never answered"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut page = b.page(None).unwrap();
        page.type_text(Some("#q"), "hello").unwrap();
        assert_eq!(
            page.eval("document.querySelector('#q').value").unwrap(),
            "hello"
        );
        assert_eq!(page.read(100).unwrap()["title"], "t");
        assert!(!page.screenshot().unwrap().is_empty());
        drop(page);
        b.close().unwrap();
    }
}
