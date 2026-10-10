//! The live view's video and input go through gliff (github.com/omacom/gliff),
//! Omarchy's remote desktop: omaspace runs `gliff-server --stdio` here, as
//! gliff's own client does over ssh, and bridges it to the browser. gliff
//! captures and encodes the screen on the GPU (CPU fallback), adapts to the
//! link, injects input and carries the clipboard; omaspace keeps the browser
//! and phone viewer, Tailscale identity, agents, take over and files.
//!
//! Wire framing (gliff-proto): `u32 body_len | u32 payload_len | CBOR body |
//! payload`, little-endian; payloads (video, cursor, clipboard) follow the
//! body. That layout never changes across gliff protocol versions.

use gliff_proto::frame::{HEADER_LEN, MAX_FRAME_BODY, MAX_FRAME_PAYLOAD};
use gliff_proto::msg::{
    ChromaMode, ClientCaps, ClientMsg, Codec, PROTOCOL_VERSION, ServerMsg, features,
};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};

/// Whether gliff-server is installed.
pub fn available() -> bool {
    crate::omarchy::which("gliff-server").is_some()
}

/// Write one message (and its payloads) in gliff's framing.
fn write_msg(w: &mut impl Write, msg: &ClientMsg, payloads: &[&[u8]]) -> anyhow::Result<()> {
    let body = minicbor::to_vec(msg)?;
    anyhow::ensure!(body.len() <= MAX_FRAME_BODY, "gliff message too large");
    let payload_len: usize = payloads.iter().map(|p| p.len()).sum();
    let mut buf = Vec::with_capacity(HEADER_LEN + body.len() + payload_len);
    buf.extend((body.len() as u32).to_le_bytes());
    buf.extend((payload_len as u32).to_le_bytes());
    buf.extend(&body);
    for p in payloads {
        buf.extend_from_slice(p);
    }
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

/// One message from the server, with its payload bytes. A message this build
/// doesn't know comes back as `None` (its payload already skipped).
fn read_msg(r: &mut impl Read) -> anyhow::Result<Option<(ServerMsg, Vec<u8>)>> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header)?;
    let body_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let payload_len = u32::from_le_bytes(header[4..].try_into().unwrap());
    anyhow::ensure!(body_len <= MAX_FRAME_BODY, "gliff body too large");
    anyhow::ensure!(payload_len <= MAX_FRAME_PAYLOAD, "gliff payload too large");
    let mut body = vec![0u8; body_len];
    r.read_exact(&mut body)?;
    let mut payload = vec![0u8; payload_len as usize];
    r.read_exact(&mut payload)?;
    match minicbor::decode::<ServerMsg>(&body) {
        Ok(m) => Ok(Some((m, payload))),
        // Newer server, newer message: skip it (gliff's compatibility rule).
        Err(e) if e.is_unknown_variant() => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// What the browser needs from a gliff session.
pub enum Event {
    /// The stream's size, chroma mode and (for 4:4:4) whether there's an
    /// aux stream; sent before the first frame and on every change.
    Config {
        width: u32,
        height: u32,
        dual: bool,
    },
    /// One encoded frame: Annex B main stream, and the aux stream (4:4:4).
    Frame {
        pts_us: u64,
        key: bool,
        main: Vec<u8>,
        aux: Vec<u8>,
    },
    /// The remote clipboard has new text (offered, then fetched).
    ClipboardText(String),
    Ended(String),
}

/// A running gliff-server for one viewer.
pub struct Session {
    /// Logical size of the screen gliff streams (pointer coordinates).
    extent: Mutex<(u32, u32)>,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pub output: String,
    clip: Mutex<ClipState>,
}

#[derive(Default)]
struct ClipState {
    /// The server's latest offer, and the id of a text request in flight.
    offer: Option<u32>,
    want: Option<u32>,
    buf: Vec<u8>,
    next_id: u32,
    /// What we offered the server (text from the browser), by serial.
    ours: Option<(u32, String)>,
}

/// How the viewer wants the screen.
pub struct Options {
    /// A private screen sized to the viewer (phones), or mirror the desktop.
    pub headless: bool,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Single 4:2:0 stream; otherwise 4:4:4 when the viewer can recombine it.
    pub low_bandwidth: bool,
    pub full_chroma: bool,
}

impl Session {
    pub fn start(opts: &Options) -> anyhow::Result<(Arc<Session>, mpsc::Receiver<Event>)> {
        let mut cmd = Command::new("gliff-server");
        cmd.arg("--stdio");
        if opts.headless {
            cmd.arg("--headless");
        }
        if opts.low_bandwidth || !opts.full_chroma {
            cmd.arg("--low-bandwidth");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("starting gliff-server: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = child.stdout.take().expect("piped");
        if let Some(err) = child.stderr.take() {
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                    let l = line.to_ascii_lowercase();
                    if l.contains("error") || l.contains("warn") {
                        eprintln!("view: gliff-server: {line}");
                    }
                }
            });
        }
        let chroma = if opts.full_chroma && !opts.low_bandwidth {
            vec![ChromaMode::Dual420, ChromaMode::Single420]
        } else {
            vec![ChromaMode::Single420]
        };
        write_msg(
            &mut stdin,
            &ClientMsg::Hello {
                version: PROTOCOL_VERSION,
                // Empty: the machine's own keymap (the viewer sends key
                // codes, and the machine maps them as if typed there).
                keymap: String::new(),
                caps: ClientCaps {
                    codecs: vec![Codec::H264],
                    max_width: 4096,
                    max_height: 4096,
                    chroma,
                    features: features(),
                },
            },
            &[],
        )?;
        let output = match read_msg(&mut stdout)? {
            Some((ServerMsg::HelloAck { session, .. }, _)) => session.output,
            Some((ServerMsg::Error { message, .. }, _)) => anyhow::bail!("gliff-server: {message}"),
            _ => anyhow::bail!("gliff-server didn't greet"),
        };
        write_msg(
            &mut stdin,
            &ClientMsg::Resize {
                width: opts.width,
                height: opts.height,
                scale: opts.scale,
            },
            &[],
        )?;
        let session = Arc::new(Session {
            extent: Mutex::new((
                (opts.width as f32 / opts.scale.max(0.5)) as u32,
                (opts.height as f32 / opts.scale.max(0.5)) as u32,
            )),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            output,
            clip: Mutex::new(ClipState::default()),
        });
        let (tx, rx) = mpsc::sync_channel(8);
        let reader = session.clone();
        std::thread::spawn(move || {
            let why = reader.pump(&mut stdout, &tx);
            let _ = tx.send(Event::Ended(why));
        });
        Ok((session, rx))
    }

    fn pump(&self, r: &mut ChildStdout, tx: &mpsc::SyncSender<Event>) -> String {
        loop {
            let (msg, payload) = match read_msg(r) {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(e) => return format!("gliff-server ended: {e}"),
            };
            let sent = match msg {
                ServerMsg::StreamConfig {
                    chroma,
                    width,
                    height,
                    scale_milli,
                    ..
                } => {
                    let scale = (scale_milli.max(1) as f64) / 1000.0;
                    *self.extent.lock().unwrap() = (
                        (f64::from(width) / scale) as u32,
                        (f64::from(height) / scale) as u32,
                    );
                    tx.send(Event::Config {
                        width,
                        height,
                        dual: chroma == ChromaMode::Dual420,
                    })
                }
                ServerMsg::VideoFrame {
                    frame_id,
                    pts_us,
                    keyframe,
                    data_len,
                    ..
                } => {
                    let split = (data_len as usize).min(payload.len());
                    let (main, aux) = payload.split_at(split);
                    // Ack every frame: gliff's rate control paces on acks.
                    let _ = self.send(&ClientMsg::FrameAck {
                        frame_id,
                        decoded_at_ms: now_ms(),
                    });
                    tx.send(Event::Frame {
                        pts_us,
                        key: keyframe,
                        main: main.to_vec(),
                        aux: aux.to_vec(),
                    })
                }
                ServerMsg::Ping { t } => {
                    let _ = self.send(&ClientMsg::Pong { t });
                    Ok(())
                }
                ServerMsg::Error { message, .. } => return format!("gliff-server: {message}"),
                ServerMsg::ClipboardOffer {
                    serial, mime_types, ..
                } => self
                    .on_offer(serial, &mime_types)
                    .map_or(Ok(()), |_| Ok(())),
                ServerMsg::ClipboardData { id, done, .. } => match self.on_data(id, &payload, done)
                {
                    Some(text) => tx.send(Event::ClipboardText(text)),
                    None => Ok(()),
                },
                ServerMsg::ClipboardRequest { id, serial, .. } => {
                    self.answer_request(id, serial);
                    Ok(())
                }
                _ => Ok(()),
            };
            if sent.is_err() {
                return "viewer gone".into();
            }
        }
    }

    fn send(&self, msg: &ClientMsg) -> anyhow::Result<()> {
        write_msg(&mut *self.stdin.lock().unwrap(), msg, &[])
    }

    fn send_with(&self, msg: &ClientMsg, payload: &[u8]) -> anyhow::Result<()> {
        write_msg(&mut *self.stdin.lock().unwrap(), msg, &[payload])
    }

    // ---- input -----------------------------------------------------------

    /// The logical size pointer positions are in.
    pub fn extent(&self) -> (u32, u32) {
        *self.extent.lock().unwrap()
    }

    /// A Linux evdev key code, pressed or released.
    pub fn key(&self, keycode: u32, pressed: bool) {
        let _ = self.send(&ClientMsg::Key { keycode, pressed });
    }

    /// Pointer at (x, y) in the stream's logical coordinates.
    pub fn motion(&self, x: f64, y: f64) {
        let _ = self.send(&ClientMsg::PointerMotion { x, y });
    }

    /// A Linux button code (272 left, 273 right, 274 middle).
    pub fn button(&self, button: u32, pressed: bool) {
        let _ = self.send(&ClientMsg::PointerButton { button, pressed });
    }

    pub fn scroll(&self, dx: f64, dy: f64) {
        use gliff_proto::msg::Axis;
        for (axis, v) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
            if v != 0.0 {
                let _ = self.send(&ClientMsg::PointerAxis {
                    axis,
                    value: v,
                    discrete: None,
                    stop: false,
                });
            }
        }
    }

    pub fn request_keyframe(&self) {
        let _ = self.send(&ClientMsg::RequestKeyframe);
    }

    // ---- clipboard (text) ------------------------------------------------

    /// The server's clipboard changed: fetch it if it's text.
    fn on_offer(&self, serial: u32, mimes: &[String]) -> Option<()> {
        use gliff_proto::clipboard::{ClipboardItem, TEXT_MIMES};
        let mime = mimes.iter().find(|m| TEXT_MIMES.contains(&m.as_str()))?;
        let mut c = self.clip.lock().unwrap();
        c.next_id += 2;
        let id = c.next_id | 1; // the client uses odd transfer ids
        c.offer = Some(serial);
        c.want = Some(id);
        c.buf.clear();
        drop(c);
        let _ = self.send(&ClientMsg::ClipboardRequest {
            id,
            serial,
            item: ClipboardItem::Mime(mime.clone()),
        });
        Some(())
    }

    fn on_data(&self, id: u32, data: &[u8], done: bool) -> Option<String> {
        let mut c = self.clip.lock().unwrap();
        if c.want != Some(id) {
            return None;
        }
        c.buf.extend_from_slice(data);
        let received = c.buf.len() as u64;
        drop(c);
        let _ = self.send(&ClientMsg::ClipboardAck { id, received });
        if !done {
            return None;
        }
        let mut c = self.clip.lock().unwrap();
        c.want = None;
        Some(String::from_utf8_lossy(&std::mem::take(&mut c.buf)).into_owned())
    }

    /// The browser copied text: offer it to the machine's clipboard.
    pub fn offer_text(&self, text: String) {
        use gliff_proto::clipboard::TEXT_MIMES;
        let mut c = self.clip.lock().unwrap();
        c.next_id += 2;
        let serial = c.next_id;
        c.ours = Some((serial, text));
        drop(c);
        let _ = self.send(&ClientMsg::ClipboardOffer {
            serial,
            mime_types: TEXT_MIMES.iter().map(|m| m.to_string()).collect(),
            files: Vec::new(),
        });
    }

    /// An app on the machine pasted: send the text we offered.
    fn answer_request(&self, id: u32, serial: u32) {
        let text = {
            let c = self.clip.lock().unwrap();
            match &c.ours {
                Some((s, t)) if *s == serial => t.clone(),
                _ => {
                    drop(c);
                    let _ = self.send(&ClientMsg::ClipboardAbort { id });
                    return;
                }
            }
        };
        let _ = self.send_with(
            &ClientMsg::ClipboardData {
                id,
                offset: 0,
                data_len: text.len() as u32,
                done: true,
            },
            text.as_bytes(),
        );
    }

    pub fn stop(&self) {
        let _ = self.send(&ClientMsg::Bye);
        let mut child = self.child.lock().unwrap();
        // Give it a moment to remove its headless output cleanly.
        for _ in 0..20 {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a real gliff-server on this machine's Hyprland: a headless
    /// session sends a stream config, then frames. `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_headless_gliff_session_streams_frames() {
        let (s, rx) = Session::start(&Options {
            headless: true,
            width: 860,
            height: 1864,
            scale: 2.0,
            low_bandwidth: true,
            full_chroma: false,
        })
        .unwrap();
        let (mut config, mut frames, mut key) = (None, 0, false);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < until && frames < 5 {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Event::Config {
                    width,
                    height,
                    dual,
                }) => config = Some((width, height, dual)),
                Ok(Event::Frame { key: k, main, .. }) => {
                    assert!(
                        main.starts_with(&[0, 0, 0, 1]) || main.starts_with(&[0, 0, 1]),
                        "Annex B"
                    );
                    key |= k;
                    frames += 1;
                }
                Ok(Event::Ended(why)) => panic!("{why}"),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        eprintln!(
            "output {} config {config:?} frames {frames} key {key}",
            s.output
        );
        s.stop();
        assert!(config.is_some(), "a stream config");
        assert!(frames > 0 && key, "frames, starting with a keyframe");
    }
}
