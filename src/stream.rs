//! GPU screen capture → hardware H.264 → access units for viewers.
//!
//! `gpu-screen-recorder` captures the Hyprland output on the GPU (damage
//! driven: no frames while nothing changes) and encodes with VAAPI/NVENC; it
//! writes MPEG-TS to stdout. This module demuxes the H.264 elementary stream
//! out of the TS and splits it into access units (one frame each, Annex B),
//! marking keyframes (IDR) so slow viewers can skip ahead.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};

#[derive(Clone, Debug)]
pub struct Frame {
    pub pts_us: u64,
    pub key: bool,
    pub data: Arc<Vec<u8>>,
}

/// What to capture and at what size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Source {
    pub output: String,
    /// `WxH+X+Y` in output pixels, for a single window; None = whole output.
    pub region: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// A virtual (headless) output: captured with wf-recorder, which can see
    /// outputs gpu-screen-recorder can't, still on the GPU (VAAPI).
    pub virtual_output: bool,
}

impl Source {
    fn program(&self) -> &'static str {
        if self.virtual_output {
            "wf-recorder"
        } else {
            "gpu-screen-recorder"
        }
    }

    fn args(&self) -> Vec<String> {
        if self.virtual_output {
            // No B-frames and a fixed QP: lowest latency, quality-targeted bits.
            // A keyframe every 0.5s (g=fps/2): a viewer that lost a frame (a
            // phone decoding slower than a workspace switch arrives) recovers
            // within half a second instead of smearing for two.
            let device = render_node();
            let gop = format!("g={}", (self.fps / 2).max(1));
            return [
                "-o",
                &self.output,
                "-c",
                "h264_vaapi",
                "-d",
                &device,
                "-r",
                &self.fps.to_string(),
                "-b",
                "0",
                "-p",
                "qp=24",
                "-p",
                &gop,
                "-m",
                "mpegts",
                "-f",
                "/dev/stdout",
                "-y",
            ]
            .map(String::from)
            .to_vec();
        }
        // `-w <WxH+X+Y>` captures that screen region (a window's rectangle).
        let mut a = vec![
            "-w".into(),
            self.region.clone().unwrap_or_else(|| self.output.clone()),
        ];
        a.extend(
            [
                "-c",
                "mpegts",
                "-k",
                "h264",
                "-fm",
                "vfr",
                "-bm",
                "qp",
                "-keyint",
                "2",
                "-cursor",
                "yes",
                "-fallback-cpu-encoding",
                "yes",
                // Hand each packet to the pipe as soon as it's muxed.
                "-ffmpeg-opts",
                "flush_packets=1;muxdelay=0;muxpreload=0",
                "-o",
                "/dev/stdout",
            ]
            .map(String::from),
        );
        // Quality-targeted: unchanged frames cost ~100 bytes, motion gets the
        // bits it needs. (CBR pads a static desktop to the full bitrate.)
        a.extend([
            "-f".into(),
            self.fps.to_string(),
            "-q".into(),
            "high".into(),
        ]);
        a.extend(["-s".into(), format!("{}x{}", self.width, self.height)]);
        a
    }
}

/// The first DRM render node (for VAAPI encoding).
fn render_node() -> String {
    std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().display().to_string())
        .filter(|p| p.contains("renderD"))
        .min()
        .unwrap_or_else(|| "/dev/dri/renderD128".into())
}

/// One running capture, shared by every viewer of the same `Source`.
pub struct Capture {
    child: Mutex<Child>,
    subscribers: Mutex<Vec<mpsc::SyncSender<Frame>>>,
    /// Last keyframe and the frames since it, so a new viewer starts at once.
    gop: Mutex<Vec<Frame>>,
}

/// An unchanged-screen frame from the encoder: a single small P slice. Not
/// sent to viewers; the last decoded picture stays on screen.
pub fn is_static(frame: &Frame) -> bool {
    !frame.key && frame.data.len() <= 160
}

impl Capture {
    pub fn start(source: &Source) -> anyhow::Result<Arc<Capture>> {
        let child = Self::spawn(source)?;
        let capture = Arc::new(Capture {
            child: Mutex::new(child),
            subscribers: Mutex::new(Vec::new()),
            gop: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&capture);
        let source = source.clone();
        std::thread::spawn(move || {
            // The recorder can die (wf-recorder intermittently segfaults on
            // a fresh headless output); restart it while anyone is watching.
            for attempt in 0.. {
                let stdout = {
                    let Some(capture) = weak.upgrade() else {
                        return;
                    };
                    let mut child = capture.child.lock().unwrap();
                    if attempt > 0 {
                        // Reap the recorder whose output just ended, so it
                        // doesn't linger as a zombie process.
                        let _ = child.wait();
                        // Its output may be gone (a phone screen removed);
                        // a few quick retries, not twenty.
                        if capture.subscribers.lock().unwrap().is_empty() || attempt > 5 {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        match Self::spawn(&source) {
                            Ok(c) => *child = c,
                            Err(_) => return,
                        }
                        eprintln!("view: restarted {} (attempt {attempt})", source.program());
                    }
                    child.stdout.take().expect("piped")
                };
                Self::pump(stdout, &weak);
            }
        });
        Ok(capture)
    }

    fn spawn(source: &Source) -> anyhow::Result<Child> {
        Ok(Command::new(source.program())
            .args(source.args())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?)
    }

    /// Demux one recorder's output until it ends.
    fn pump(mut reader: std::process::ChildStdout, weak: &std::sync::Weak<Capture>) {
        let mut demux = TsDemux::default();
        let mut au = AccessUnits;
        let mut buf = [0u8; 188 * 64];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            let whole = carry.len() / 188 * 188;
            for packet in carry[..whole].chunks(188) {
                for (pts, es) in demux.push(packet) {
                    for frame in au.push(pts, &es) {
                        let Some(capture) = weak.upgrade() else {
                            return;
                        };
                        capture.publish(frame);
                    }
                }
            }
            carry.drain(..whole);
        }
    }

    fn publish(&self, frame: Frame) {
        {
            let mut gop = self.gop.lock().unwrap();
            if frame.key {
                gop.clear();
            }
            if frame.key || !gop.is_empty() {
                gop.push(frame.clone());
            }
        }
        // A subscriber whose queue is full is dropped from this frame; the
        // viewer resynchronises at the next keyframe (see `subscribe`).
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| match tx.try_send(frame.clone()) {
                Ok(()) => true,
                Err(mpsc::TrySendError::Full(_)) => true,
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            });
    }

    /// Frames for one viewer, starting with the current GOP so the picture
    /// appears immediately.
    pub fn subscribe(&self) -> mpsc::Receiver<Frame> {
        let (tx, rx) = mpsc::sync_channel(90);
        for frame in self.gop.lock().unwrap().iter() {
            let _ = tx.try_send(frame.clone());
        }
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    pub fn viewers(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    pub fn stop(&self) {
        self.subscribers.lock().unwrap().clear();
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Captures keyed by source, started on first viewer, stopped after the last.
#[derive(Default)]
pub struct Captures {
    running: Mutex<HashMap<Source, Arc<Capture>>>,
}

impl Captures {
    pub fn get(&self, source: &Source) -> anyhow::Result<Arc<Capture>> {
        let mut running = self.running.lock().unwrap();
        running.retain(|_, c| {
            let alive = c.viewers() > 0 || c.gop.lock().unwrap().is_empty();
            if !alive {
                c.stop();
            }
            alive
        });
        if let Some(c) = running.get(source) {
            return Ok(c.clone());
        }
        let c = Capture::start(source)?;
        running.insert(source.clone(), c.clone());
        Ok(c)
    }

    pub fn stop_idle(&self) {
        self.running.lock().unwrap().retain(|_, c| {
            let alive = c.viewers() > 0;
            if !alive {
                c.stop();
            }
            alive
        });
    }
}

/// MPEG-TS → H.264 PES payloads with their PTS (90 kHz → µs).
#[derive(Default)]
struct TsDemux {
    video_pid: Option<u16>,
    pmt_pid: Option<u16>,
    pes: Vec<u8>,
    pts: u64,
}

impl TsDemux {
    fn push(&mut self, p: &[u8]) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        if p.len() != 188 || p[0] != 0x47 {
            return out;
        }
        let start = p[1] & 0x40 != 0;
        let pid = (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2]);
        let afc = (p[3] >> 4) & 3;
        let mut i = 4;
        if afc & 2 != 0 {
            i += 1 + p[4] as usize;
        }
        if afc & 1 == 0 || i >= 188 {
            return out;
        }
        let payload = &p[i..];
        if pid == 0 && start {
            // PAT: first program's PMT pid.
            let s = &payload[1 + payload[0] as usize..];
            if s.len() >= 12 {
                self.pmt_pid = Some((u16::from(s[10] & 0x1f) << 8) | u16::from(s[11]));
            }
        } else if Some(pid) == self.pmt_pid && start {
            let s = &payload[1 + payload[0] as usize..];
            if s.len() < 12 {
                return out;
            }
            let section_len = ((usize::from(s[1] & 0x0f) << 8) | usize::from(s[2]))
                .min(s.len().saturating_sub(3));
            let info_len = (usize::from(s[10] & 0x0f) << 8) | usize::from(s[11]);
            let mut j = 12 + info_len;
            let end = 3 + section_len - 4;
            while j + 5 <= end {
                let stream_type = s[j];
                let es_pid = (u16::from(s[j + 1] & 0x1f) << 8) | u16::from(s[j + 2]);
                let es_info = (usize::from(s[j + 3] & 0x0f) << 8) | usize::from(s[j + 4]);
                if stream_type == 0x1b {
                    self.video_pid = Some(es_pid);
                }
                j += 5 + es_info;
            }
        } else if Some(pid) == self.video_pid {
            if start {
                if !self.pes.is_empty() {
                    out.push((self.pts, std::mem::take(&mut self.pes)));
                }
                // PES header: 00 00 01 sid len(2) flags(2) hdr_len, PTS if flagged.
                if payload.len() >= 9 && payload[..3] == [0, 0, 1] {
                    let hdr_len = payload[8] as usize;
                    if payload[7] & 0x80 != 0 && payload.len() >= 14 {
                        let b = &payload[9..14];
                        let pts = (u64::from(b[0] >> 1 & 7) << 30)
                            | (u64::from(b[1]) << 22)
                            | (u64::from(b[2] >> 1) << 15)
                            | (u64::from(b[3]) << 7)
                            | u64::from(b[4] >> 1);
                        self.pts = pts * 100 / 9;
                    }
                    self.pes
                        .extend_from_slice(payload.get(9 + hdr_len..).unwrap_or_default());
                }
            } else {
                self.pes.extend_from_slice(payload);
            }
        }
        out
    }
}

/// Annex B elementary stream → access units. gpu-screen-recorder writes one
/// access unit per PES, so each PES is a frame; this also marks IDR frames.
#[derive(Default)]
struct AccessUnits;

impl AccessUnits {
    fn push(&mut self, pts: u64, es: &[u8]) -> Vec<Frame> {
        if es.is_empty() {
            return Vec::new();
        }
        vec![Frame {
            pts_us: pts,
            key: has_idr(es),
            data: Arc::new(es.to_vec()),
        }]
    }
}

/// Whether an Annex B buffer contains an IDR slice (NAL type 5).
pub fn has_idr(es: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < es.len() {
        if es[i] == 0 && es[i + 1] == 0 && (es[i + 2] == 1 || (es[i + 2] == 0 && es[i + 3] == 1)) {
            let start = if es[i + 2] == 1 { i + 3 } else { i + 4 };
            if es.get(start).is_some_and(|b| b & 0x1f == 5) {
                return true;
            }
            i = start;
        } else {
            i += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idr_frames_are_found_after_either_start_code() {
        assert!(has_idr(&[0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x65, 9]));
        assert!(!has_idr(&[0, 0, 0, 1, 0x41, 1, 2, 3]));
    }

    /// A real capture, split from a gpu-screen-recorder TS file:
    /// `OMASPACE_TS=/tmp/gsr.ts cargo test real_ts`.
    #[test]
    fn real_ts_splits_into_frames_starting_with_a_keyframe() {
        let Ok(path) = std::env::var("OMASPACE_TS") else {
            return;
        };
        let data = std::fs::read(path).unwrap();
        let (mut demux, mut au) = (TsDemux::default(), AccessUnits);
        let frames: Vec<Frame> = data
            .chunks(188)
            .flat_map(|p| demux.push(p))
            .flat_map(|(pts, es)| au.push(pts, &es))
            .collect();
        eprintln!(
            "frames={} keyframes={}",
            frames.len(),
            frames.iter().filter(|f| f.key).count()
        );
        assert!(frames.len() > 10);
        assert!(frames[0].key);
        assert!(frames.windows(2).all(|w| w[1].pts_us >= w[0].pts_us));
    }
}
