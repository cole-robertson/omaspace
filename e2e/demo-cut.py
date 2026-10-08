#!/usr/bin/env python3
"""Cut the demo recorded by demo.mjs into one captioned video.

    python3 demo-cut.py /tmp/demo /tmp/omaspace-demo.mp4            1920x1080, for screens
    python3 demo-cut.py /tmp/demo /tmp/omaspace-demo-phone.mp4 --vertical
                                                                     1080x1920, to watch on a phone

Each scene's frames are placed at their recorded times (a variable-rate
screencast becomes steady 30 fps by holding the latest frame), framed on a
dark background, with the scene's captions underneath.
"""
import json, os, subprocess, sys, tempfile

SRC, DST = sys.argv[1], sys.argv[2]
VERTICAL = "--vertical" in sys.argv
W, H, FPS = (1080, 1920, 30) if VERTICAL else (1920, 1080, 30)
BG = "#16161e"
FONT = "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf"
ACCENT = "#7aa2f7"


def concat_list(scene, path):
    """An ffconcat file that shows each frame for exactly as long as it was on screen."""
    j = json.load(open(f"{SRC}/{scene}/timeline.json"))
    frames = j["frames"]
    with open(path, "w") as f:
        f.write("ffconcat version 1.0\n")
        for a, b in zip(frames, frames[1:] + [{"t": frames[-1]["t"] + 1500}]):
            f.write(f"file '{a['file']}'\nduration {max(0.001, (b['t'] - a['t']) / 1000):.3f}\n")
        f.write(f"file '{frames[-1]['file']}'\n")
    return j, (frames[-1]["t"] + 1500) / 1000


def esc(t):
    return t.replace("\\", "\\\\").replace(":", "\\:").replace("'", "’").replace("%", "\\%")


def captions(marks, total, y):
    """drawtext filters: each caption from its mark to the next."""
    out = []
    for m, n in zip(marks, marks[1:] + [{"t": total * 1000}]):
        a, b = m["t"] / 1000 + 0.15, n["t"] / 1000
        fade = f"if(lt(t,{a}+0.25),(t-{a})/0.25,if(gt(t,{b}-0.25),({b}-t)/0.25,1))"
        out.append(
            f"drawtext=fontfile={FONT}:text='{esc(m['text'])}':fontsize=40:fontcolor=white:"
            f"x=(w-text_w)/2:y={y}:alpha='{fade}':enable='between(t,{a},{b})'"
        )
    return out


def wrap(text, width):
    words, lines, cur = text.split(), [], ""
    for w in words:
        if len(cur) + len(w) + 1 > width and cur:
            lines.append(cur); cur = w
        else:
            cur = (cur + " " + w).strip()
    lines.append(cur)
    return lines


def caption_lines(marks, total, x, y_mid, size, width, step):
    """Wrapped captions, centred on y_mid; x is an expression ("(w-text_w)/2") or a number."""
    out = []
    for m, n in zip(marks, marks[1:] + [{"t": total * 1000}]):
        a, b = m["t"] / 1000 + 0.15, n["t"] / 1000
        fade = f"if(lt(t,{a}+0.25),(t-{a})/0.25,if(gt(t,{b}-0.25),({b}-t)/0.25,1))"
        lines = wrap(m["text"], width)
        for k, line in enumerate(lines):
            out.append(
                f"drawtext=fontfile={FONT}:text='{esc(line)}':fontsize={size}:fontcolor=white:"
                f"x={x}:y={round(y_mid - step * len(lines) / 2 + k * step)}:alpha='{fade}':enable='between(t,{a},{b})'"
            )
    return out


def title(text, sub, seconds, out):
    subs = wrap(sub, 28) if VERTICAL else [sub]
    vf = (
        f"drawtext=fontfile={FONT}:text='{esc(text)}':fontsize=110:fontcolor={ACCENT}:x=(w-text_w)/2:y=(h/2)-120,"
        + "".join(f"drawtext=fontfile={FONT}:text='{esc(l)}':fontsize={56 if VERTICAL else 40}:fontcolor=white:x=(w-text_w)/2:y=(h/2)+{30 + i * 76},"
                  for i, l in enumerate(subs)) +
        f"fade=in:0:12,fade=out:st={seconds - 0.5}:d=0.5"
    )
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"color=c={BG}:s={W}x{H}:r={FPS}:d={seconds}",
                    "-vf", vf, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "slow", "-crf", "20", out], check=True)


tmp = tempfile.mkdtemp()
parts = []

title("omaspace", "Your Omarchy desktop, from anywhere on your tailnet", 3.0, f"{tmp}/0.mp4")
parts.append(f"{tmp}/0.mp4")

# Laptop scene: the browser window, big, with captions below.
j, total = concat_list("laptop", f"{tmp}/laptop.txt")
if VERTICAL:
    # Full width, captions above and below.
    lw = W - 40; lh = round(lw * j["size"]["h"] / j["size"]["w"]) // 2 * 2
    top = (H - lh) // 2
    vf = ",".join([
        f"fps={FPS}", f"scale={lw}:{lh}:flags=lanczos",
        f"pad={W}:{H}:(ow-iw)/2:{top}:color={BG}",
        f"drawbox=x=18:y={top - 2}:w={lw + 4}:h={lh + 4}:color={ACCENT}@0.6:t=2",
        *caption_lines(j["marks"], total, "(w-text_w)/2", top + lh + 200, 58, 26, 80),
        f"fade=in:0:9,fade=out:st={total - 0.4}:d=0.4",
    ])
else:
    lw = 1500; lh = round(lw * j["size"]["h"] / j["size"]["w"])
    vf = ",".join([
        f"fps={FPS}", f"scale={lw}:{lh}:flags=lanczos",
        f"pad={W}:{H}:(ow-iw)/2:32:color={BG}",
        "drawbox=x=(iw-%d)/2-2:y=30:w=%d:h=%d:color=%s@0.6:t=2" % (lw, lw + 4, lh + 4, ACCENT),
        *captions(j["marks"], total, 32 + lh + 28),
        f"fade=in:0:9,fade=out:st={total - 0.4}:d=0.4",
    ])
subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/laptop.txt",
                "-vf", vf, "-t", f"{total:.2f}", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "slow", "-crf", "20",
                f"{tmp}/1.mp4"], check=True)
parts.append(f"{tmp}/1.mp4")

# Phone scene. Landscape: the phone on the left, captions to its right.
# Vertical: the phone fills the frame, with captions in a band at the bottom.
j, total = concat_list("phone", f"{tmp}/phone.txt")
src_h = j["frames"] and j["size"]["h"]
if VERTICAL:
    cap_band = 260
    ph = H - cap_band - 40; pw = round(ph * 430 / 739) // 2 * 2
    if pw > W - 40:
        pw = (W - 40) // 2 * 2; ph = round(pw * 739 / 430) // 2 * 2
    px, py = (W - pw) // 2, 20
    vf = ",".join([
        # The phone was captured at ~17 fps (full-resolution screenshots):
        # blend between frames to a steady 30 so swipes and swaps don't judder.
        f"framerate=fps={FPS}:interp_start=0:interp_end=255:scene=8", f"scale={pw}:{ph}:flags=lanczos",
        f"pad={W}:{H}:{px}:{py}:color={BG}",
        f"drawbox=x={px - 3}:y={py - 3}:w={pw + 6}:h={ph + 6}:color={ACCENT}@0.6:t=3",
        *caption_lines(j["marks"], total, "(w-text_w)/2", py + ph + cap_band // 2 + 10, 54, 30, 72),
        f"fade=in:0:9,fade=out:st={total - 0.6}:d=0.6",
    ])
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/phone.txt",
                    "-vf", vf, "-t", f"{total:.2f}", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "slow", "-crf", "20",
                    f"{tmp}/2.mp4"], check=True)
    parts.append(f"{tmp}/2.mp4")
ph = 1000; pw = round(ph * 430 / 739) // 2 * 2
px = 360
cap = []
for m, n in zip(j["marks"], j["marks"][1:] + [{"t": total * 1000}]):
    a, b = m["t"] / 1000 + 0.15, n["t"] / 1000
    fade = f"if(lt(t,{a}+0.25),(t-{a})/0.25,if(gt(t,{b}-0.25),({b}-t)/0.25,1))"
    # Wrap long captions onto two lines at a word boundary near the middle.
    words, lines, cur = m["text"].split(), [], ""
    for w in words:
        if len(cur) + len(w) + 1 > 30 and cur:
            lines.append(cur); cur = w
        else:
            cur = (cur + " " + w).strip()
    lines.append(cur)
    for k, line in enumerate(lines):
        cap.append(
            f"drawtext=fontfile={FONT}:text='{esc(line)}':fontsize=46:fontcolor=white:"
            f"x={px + pw + 110}:y={H // 2 - 40 * len(lines) + k * 72}:alpha='{fade}':enable='between(t,{a},{b})'"
        )
if not VERTICAL:
    vf = ",".join([
        f"fps={FPS}", f"scale={pw}:{ph}:flags=lanczos",
        f"pad={W}:{H}:{px}:(oh-ih)/2:color={BG}",
        f"drawbox=x={px - 3}:y=(ih-{ph})/2-3:w={pw + 6}:h={ph + 6}:color={ACCENT}@0.6:t=3",
        *cap,
        f"fade=in:0:9,fade=out:st={total - 0.6}:d=0.6",
    ])
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/phone.txt",
                    "-vf", vf, "-t", f"{total:.2f}", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "slow", "-crf", "20",
                    f"{tmp}/2.mp4"], check=True)
    parts.append(f"{tmp}/2.mp4")

with open(f"{tmp}/all.txt", "w") as f:
    for p in parts:
        f.write(f"file '{p}'\n")
subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", f"{tmp}/all.txt",
                "-c", "copy", "-movflags", "+faststart", DST], check=True)
print(DST)
