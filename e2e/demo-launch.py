#!/usr/bin/env python3
"""The omaspace launch video, cut from the footage demo.mjs records.

    python3 demo-launch.py /tmp/demo /tmp/omaspace-launch.mp4            1920x1080
    python3 demo-launch.py /tmp/demo /tmp/omaspace-launch-phone.mp4 --vertical

Rendered frame by frame (Pillow) and piped to ffmpeg: a glowing gradient
stage, device frames that ease in and zoom onto the action, big kinetic
titles, and an outro with the install line. Timing is driven by the
caption marks demo.mjs wrote, so a re-recorded take re-cuts itself.
"""
import json, math, subprocess, sys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

SRC, DST = sys.argv[1], sys.argv[2]
VERTICAL = "--vertical" in sys.argv
SHORT = "--short" in sys.argv  # the 15-second cut
TEASER = "--teaser" in sys.argv  # the ~20-second teaser
PUSH_AFTER = set()  # scene indices that hand over with a sideways push, not a fade
CTA = "Open source · coming soon"
W, H = (1080, 1920) if VERTICAL else (1920, 1080)
FPS = 30

INK = (232, 236, 255)
DIM = (150, 158, 196)
ACCENT = (122, 162, 247)   # Tokyo Night blue (Omarchy's default theme)
AGENT = (224, 175, 104)    # agent yellow, as in the live view
GOOD = (158, 206, 106)
BG0, BG1 = (10, 11, 20), (22, 22, 38)

BLACK = "/usr/share/fonts/noto/NotoSans-Black.ttf"
BOLD = "/usr/share/fonts/noto/NotoSans-Bold.ttf"
REG = "/usr/share/fonts/noto/NotoSans-Regular.ttf"
MONO = "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf"
_fonts = {}


def font(path, size):
    key = (path, size)
    if key not in _fonts:
        _fonts[key] = ImageFont.truetype(path, size)
    return _fonts[key]


def ease(t):  # ease-out cubic, t in 0..1
    t = max(0.0, min(1.0, t))
    return 1 - (1 - t) ** 3


def ease_io(t):
    t = max(0.0, min(1.0, t))
    return 3 * t * t - 2 * t * t * t


def lerp(a, b, t):
    return a + (b - a) * t


# ---- footage ---------------------------------------------------------------

class Footage:
    """A recorded scene: frames at their recorded times, and caption marks."""

    def __init__(self, scene):
        j = json.load(open(f"{SRC}/{scene}/timeline.json"))
        self.frames = j["frames"]
        self.touches = j.get("touches", [])
        self.viewport = j.get("viewport") or {"width": 430, "height": 932}
        self.marks = {m["text"].split()[0] + " " + m["text"].split()[1]: m["t"] / 1000 for m in j["marks"]}
        self.end = self.frames[-1]["t"] / 1000
        self._cache = (None, None)

    def mark(self, prefix):
        for k, v in self.marks.items():
            if k.startswith(prefix):
                return v
        raise KeyError(prefix)

    def at(self, t):
        """The frame on screen at time t (seconds into the recording)."""
        ms = t * 1000
        lo, hi = 0, len(self.frames) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if self.frames[mid]["t"] <= ms:
                lo = mid
            else:
                hi = mid - 1
        f = self.frames[lo]["file"]
        if self._cache[0] != f:
            self._cache = (f, Image.open(f).convert("RGB"))
        return self._cache[1]


LAP = Footage("laptop")
PHONE = Footage("phone")


# ---- drawing helpers ---------------------------------------------------------

def background(t):
    """A dark stage with two slow drifting glows."""
    bg = Image.new("RGB", (W, H), BG0)
    g = Image.new("RGB", (W // 8, H // 8), BG0)
    d = ImageDraw.Draw(g)
    w8, h8 = W // 8, H // 8
    for (cx, cy, r, col) in (
        (0.25 + 0.05 * math.sin(t * 0.3), 0.3 + 0.04 * math.cos(t * 0.25), 0.55, (40, 52, 110)),
        (0.8 + 0.04 * math.cos(t * 0.27), 0.75 + 0.05 * math.sin(t * 0.22), 0.5, (70, 36, 96)),
    ):
        x, y, rr = cx * w8, cy * h8, r * max(w8, h8)
        d.ellipse((x - rr, y - rr, x + rr, y + rr), fill=col)
    g = g.filter(ImageFilter.GaussianBlur(max(w8, h8) * 0.18)).resize((W, H), Image.BILINEAR)
    return Image.blend(bg, g, 0.85)


def rounded(img, radius):
    m = Image.new("L", img.size, 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, img.size[0] - 1, img.size[1] - 1), radius, fill=255)
    return m


def shadow_glow(canvas, box, radius, color, strength):
    """A soft coloured glow behind a device."""
    x0, y0, x1, y1 = box
    pad = 80
    layer = Image.new("RGBA", (x1 - x0 + 2 * pad, y1 - y0 + 2 * pad), (0, 0, 0, 0))
    ImageDraw.Draw(layer).rounded_rectangle((pad, pad, pad + x1 - x0, pad + y1 - y0), radius, fill=color + (int(150 * strength),))
    layer = layer.filter(ImageFilter.GaussianBlur(38))
    canvas.alpha_composite(layer, (x0 - pad, y0 - pad))


def laptop_frame(canvas, shot, box, alpha=1.0):
    """A laptop: rounded screen with a thin bezel and a base."""
    x0, y0, x1, y1 = [int(v) for v in box]
    w, h = x1 - x0, y1 - y0
    if w < 8 or h < 8:
        return
    shadow_glow(canvas, (x0, y0, x1, y1), 22, ACCENT, 0.45 * alpha)
    bez = max(6, w // 90)
    body = Image.new("RGBA", (w + 2 * bez, h + 2 * bez), (24, 26, 40, int(255 * alpha)))
    canvas.paste(body, (x0 - bez, y0 - bez), rounded(body, 22 + bez))
    scr = shot.resize((w, h), Image.LANCZOS).convert("RGBA")
    scr.putalpha(rounded(scr, 14).point(lambda v: int(v * alpha)))
    canvas.alpha_composite(scr, (x0, y0))
    # base
    bw, bh = int((w + 2 * bez) * 1.12), max(10, h // 28)
    base = Image.new("RGBA", (bw, bh), (0, 0, 0, 0))
    ImageDraw.Draw(base).rounded_rectangle((0, 0, bw - 1, bh - 1), bh // 2, fill=(46, 49, 70, int(255 * alpha)))
    canvas.alpha_composite(base, (x0 - bez - (bw - w - 2 * bez) // 2, y1 + bez - 2))


def window_frame(canvas, shot, box, alpha=1.0):
    """A plain rounded window with a glow (portrait laptop scenes)."""
    x0, y0, x1, y1 = [int(v) for v in box]
    w, h = x1 - x0, y1 - y0
    if w < 8:
        return
    shadow_glow(canvas, (x0, y0, x1, y1), 26, ACCENT, 0.5 * alpha)
    scr = shot.resize((w, h), Image.LANCZOS).convert("RGBA")
    scr.putalpha(rounded(scr, 26).point(lambda v: int(v * alpha)))
    canvas.alpha_composite(scr, (x0, y0))
    edge = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(edge).rounded_rectangle((0, 0, w - 1, h - 1), 26, outline=(70, 76, 110, int(255 * alpha)), width=2)
    canvas.alpha_composite(edge, (x0, y0))


PHONE_ASPECT = 932 / 430  # iPhone 15 Pro Max, points (19.5:9)


def status_bar(canvas, x0, y0, w, alpha):
    """iOS status bar over the top safe area: time left, signal/wifi/battery right."""
    k = w / 430  # points → pixels
    f = font(BOLD, max(8, int(17 * k)))
    col = (255, 255, 255, int(255 * alpha))
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    cy = y0 + 31 * k
    d.text((x0 + 48 * k, cy), "9:41", font=f, fill=col, anchor="mm")
    # signal bars
    bx = x0 + w - 96 * k
    for i in range(4):
        bh = (4 + 2.6 * i) * k
        d.rounded_rectangle((bx + i * 5.2 * k, cy + 5 * k - bh, bx + i * 5.2 * k + 3.4 * k, cy + 5 * k), 1 * k, fill=col)
    # wifi: three arcs
    wx, wy = x0 + w - 62 * k, cy + 5 * k
    for i, r in enumerate((10, 6.5, 3)):
        rr = r * k
        if i < 2:
            d.arc((wx - rr, wy - rr, wx + rr, wy + rr), 225, 315, fill=col, width=max(1, int(2.2 * k)))
        else:
            d.pieslice((wx - rr, wy - rr, wx + rr, wy + rr), 225, 315, fill=col)
    # battery
    ex = x0 + w - 44 * k
    d.rounded_rectangle((ex, cy - 6 * k, ex + 25 * k, cy + 6 * k), 3.5 * k, outline=col, width=max(1, int(1.2 * k)))
    d.rounded_rectangle((ex + 2 * k, cy - 4 * k, ex + 19 * k, cy + 4 * k), 2 * k, fill=col)
    d.rounded_rectangle((ex + 26.5 * k, cy - 2 * k, ex + 28 * k, cy + 2 * k), 1 * k, fill=col)
    canvas.alpha_composite(layer)


def phone_frame(canvas, shot, box, alpha=1.0):
    """An iPhone 15 Pro: thin titanium bezel, 55pt corners, Dynamic Island,
    status bar and home indicator drawn over the app's safe areas."""
    x0, y0, x1, y1 = [int(v) for v in box]
    w, h = x1 - x0, y1 - y0
    if w < 8:
        return
    k = w / 430
    shadow_glow(canvas, (x0, y0, x1, y1), int(55 * k), ACCENT, 0.5 * alpha)
    bez = max(5, int(13 * k))
    r = int(55 * k)
    # Titanium frame: a lighter rim, then the black bezel.
    rim = Image.new("RGBA", (w + 2 * bez + 6, h + 2 * bez + 6), (92, 96, 112, int(255 * alpha)))
    canvas.paste(rim, (x0 - bez - 3, y0 - bez - 3), rounded(rim, r + bez + 3))
    body = Image.new("RGBA", (w + 2 * bez, h + 2 * bez), (6, 6, 9, int(255 * alpha)))
    canvas.paste(body, (x0 - bez, y0 - bez), rounded(body, r + bez))
    # Side buttons.
    btn = (78, 82, 98, int(255 * alpha))
    d = ImageDraw.Draw(canvas)
    d.rounded_rectangle((x1 + bez + 2, y0 + 190 * k, x1 + bez + 6, y0 + 290 * k), 2, fill=btn)
    for (a, b) in ((150, 190), (215, 275), (290, 350)):
        d.rounded_rectangle((x0 - bez - 6, y0 + a * k, x0 - bez - 2, y0 + b * k), 2, fill=btn)
    scr = shot.resize((w, h), Image.LANCZOS).convert("RGBA")
    scr.putalpha(rounded(scr, r).point(lambda v: int(v * alpha)))
    canvas.alpha_composite(scr, (x0, y0))
    status_bar(canvas, x0, y0, w, alpha)
    # Dynamic Island.
    iw, ih = int(126 * k), int(37 * k)
    isl = Image.new("RGBA", (iw, ih), (0, 0, 0, 0))
    ImageDraw.Draw(isl).rounded_rectangle((0, 0, iw - 1, ih - 1), ih // 2, fill=(0, 0, 0, int(255 * alpha)))
    canvas.alpha_composite(isl, (x0 + (w - iw) // 2, y0 + int(11 * k)))
    # Home indicator.
    hw, hh = int(140 * k), max(3, int(5 * k))
    ind = Image.new("RGBA", (hw, hh), (0, 0, 0, 0))
    ImageDraw.Draw(ind).rounded_rectangle((0, 0, hw - 1, hh - 1), hh // 2, fill=(255, 255, 255, int(220 * alpha)))
    canvas.alpha_composite(ind, (x0 + (w - hw) // 2, y1 - int(13 * k) - hh))


def text(canvas, s, xy, fnt, color, alpha=1.0, anchor="la", glow=None):
    if alpha <= 0:
        return
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    if glow:
        g = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
        ImageDraw.Draw(g).text(xy, s, font=fnt, fill=glow + (int(200 * alpha),), anchor=anchor)
        canvas.alpha_composite(g.filter(ImageFilter.GaussianBlur(18)))
    d.text(xy, s, font=fnt, fill=color + (int(255 * alpha),), anchor=anchor)
    canvas.alpha_composite(layer)


def kinetic(canvas, lines, t, x, y, size, align="left", color=INK, accent_word=None, lead=0.0, stagger=0.07, line_gap=1.12):
    """Words that rise and fade in one after another; the accent word in blue."""
    f = font(BLACK, size)
    words_total = 0
    for li, line in enumerate(lines):
        words = line.split()
        widths = [f.getlength(w + " ") for w in words]
        lw = sum(widths) - f.getlength(" ")
        cx = x - lw / 2 if align == "center" else x
        for wi, w in enumerate(words):
            k = ease((t - lead - stagger * words_total) / 0.45)
            words_total += 1
            if k <= 0:
                cx += widths[wi]
                continue
            col = ACCENT if accent_word and w.strip(".,!?").lower() == accent_word else color
            text(canvas, w, (cx, y + li * size * line_gap + (1 - k) * size * 0.5), f, col, alpha=k)
            cx += widths[wi]


def chip(canvas, label, t, xy, color=ACCENT, size=30):
    """A small pill label, e.g. ON YOUR PHONE."""
    k = ease(t / 0.4)
    if k <= 0:
        return
    f = font(BOLD, size)
    tw = f.getlength(label)
    x, y = xy
    pad = size * 0.55
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.rounded_rectangle((x, y, x + tw + 2 * pad, y + size * 1.7), size, fill=color + (int(40 * k),), outline=color + (int(220 * k),), width=2)
    d.text((x + pad, y + size * 0.85), label, font=f, fill=color + (int(255 * k),), anchor="lm")
    canvas.alpha_composite(layer, (0, int((1 - k) * 12)))


def crop_zoom(shot, cx, cy, z):
    """Zoom into a shot around (cx, cy) in 0..1, keeping its size."""
    if z <= 1.001:
        return shot
    w, h = shot.size
    cw, ch = w / z, h / z
    x0 = min(max(cx * w - cw / 2, 0), w - cw)
    y0 = min(max(cy * h - ch / 2, 0), h - ch)
    return shot.crop((int(x0), int(y0), int(x0 + cw), int(y0 + ch))).resize((w, h), Image.LANCZOS)


# ---- the timeline --------------------------------------------------------------
# Few cuts: the laptop and the phone each play as one continuous take, and
# their headlines change in place over the footage. Each scene: (seconds,
# draw(canvas, t_local)).

scenes = []


def scene(seconds):
    def add(fn):
        scenes.append((seconds, fn))
        return fn
    return add


L = (W > H)  # landscape


@scene(2.8)
def hook(c, t):
    s = 128 if L else 112
    lines = ["Your Omarchy desktop,"] if L else ["Your Omarchy", "desktop,"]
    y = H / 2 - s * (0.95 if L else 1.6)
    kinetic(c, lines, t, W / 2, y, s, align="center")
    kinetic(c, ["for you and your agents."] if L else ["for you and", "your agents."], t, W / 2, y + s * 1.12 * len(lines), s,
            align="center", accent_word="agents", lead=0.35)


def device_box(kind, k):
    """Where the device sits, easing up from below as k goes 0 → 1."""
    if kind == "laptop":
        if L:
            # Nearly the full frame; the headline sits in a band above it.
            h = 860; w = int(h * 1440 / 900); x = (W - w) / 2; y = 175
        else:
            # A tall window onto the desktop: the footage is cropped to this
            # shape by laptop_scene, so it fills the frame like the phone does.
            w = W - 60; h = 1500; x = 30; y = H - h - 70
    else:
        if L:
            # Almost the full height, right of centre; headline on the left.
            h = 1010; w = int(h / PHONE_ASPECT); x = W * 0.66 - w / 2; y = (H - h) / 2
        else:
            # As tall as fits under a two-line headline.
            h = 1520; w = int(h / PHONE_ASPECT); x = (W - w) / 2; y = H - h - 50
    dy = (1 - k) * 120
    return (x, y + dy, x + w, y + h + dy)


def headline(c, t, chip_label, lines, accent=None, chip_color=ACCENT):
    if L:
        # One line: chip on the left, the headline beside it.
        line = " ".join(lines)
        chip(c, chip_label, t, (W / 2 - 790, 52), color=chip_color, size=26)
        kinetic(c, [line], t - 0.1, W / 2 - 790 + font(BOLD, 26).getlength(chip_label) + 72, 38, 76, accent_word=accent)
    else:
        chip(c, chip_label, t, (50, 55), color=chip_color, size=30)
        kinetic(c, lines, t - 0.1, 50, 125, 74, accent_word=accent, line_gap=1.05)



def beat_at(beats, t):
    """The headline beat showing at time t, and how long it's been showing."""
    cur = beats[0]
    for b in beats:
        if t >= b[0]:
            cur = b
    return cur, t - cur[0]


def caption_band(c, beats, t, layout):
    """Headlines that change in place: the old one fades out as the next rises."""
    if t < beats[0][0]:
        return
    b, since = beat_at(beats, t)
    idx = beats.index(b)
    out = min(1.0, max(0.0, (beats[idx + 1][0] - t) / 0.25)) if idx + 1 < len(beats) else 1.0
    _, label, lines, accent, color = b
    layer = Image.new("RGBA", c.size, (0, 0, 0, 0))
    if layout == "top":
        if L:
            line = " ".join(lines)
            x = W / 2 - 790 + font(BOLD, 26).getlength(label) + 72
            fits = font(BLACK, 76).getlength(line) <= W / 2 + 790 - x
            chip(layer, label, since, (W / 2 - 790, 52), color=color, size=26)
            if fits:
                kinetic(layer, [line], since - 0.05, x, 38, 76, accent_word=accent)
            else:
                kinetic(layer, [line], since - 0.05, x, 46, 60, accent_word=accent)
        else:
            chip(layer, label, since, (50, 55), color=color, size=30)
            kinetic(layer, lines, since - 0.05, 50, 125, 74, accent_word=accent, line_gap=1.05)
    else:  # side (landscape phone)
        room = W * 0.66 - 1010 / PHONE_ASPECT / 2 - 110 - 70
        size = 88
        while size > 56 and max(font(BLACK, size).getlength(l) for l in lines) > room:
            size -= 4
        y0 = H / 2 - (70 + size * 1.12 * len(lines)) / 2
        chip(layer, label, since, (110, y0), color=color, size=30)
        kinetic(layer, lines, since - 0.05, 110, y0 + 80, size, accent_word=accent)
    if out < 1.0:
        a = layer.split()[3].point(lambda v: int(v * out))
        layer.putalpha(a)
    c.alpha_composite(layer)


def speed_map(start, segments):
    """Video time → footage time. `segments` are (footage_from, footage_to,
    speed); outside them footage plays at 1x. Returns (map, video_seconds)."""
    pieces, t_src, t_vid = [], start, 0.0
    for a, b, spd in sorted(segments):
        if a > t_src:
            pieces.append((t_vid, t_src, 1.0)); t_vid += a - t_src; t_src = a
        pieces.append((t_vid, t_src, spd)); t_vid += (b - t_src) / spd; t_src = b
    pieces.append((t_vid, t_src, 1.0))

    def m(tv):
        cur = pieces[0]
        for pc in pieces:
            if tv >= pc[0]:
                cur = pc
        return cur[1] + (tv - cur[0]) * cur[2]

    def inv(ts):
        cur = pieces[0]
        for pc in pieces:
            if ts >= pc[1]:
                cur = pc
        return cur[0] + (ts - cur[1]) / cur[2]
    return m, inv


def fingers(canvas, footage, src_t, box):
    """A tap ripple / held-finger dot over the device, from the recorded touches."""
    vw, vh = footage.viewport["width"], footage.viewport["height"]
    x0, y0, x1, y1 = box
    kx, ky = (x1 - x0) / vw, (y1 - y0) / vh
    ms = src_t * 1000
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    r0 = 34 * kx
    # Held finger: last event up to now is a down/move.
    held = None
    for ev in footage.touches:
        if ev["t"] > ms:
            break
        held = ev if ev["phase"] in ("down", "move") else None
    if held and ms - held["t"] < 1500:
        x, y = x0 + held["x"] * kx, y0 + held["y"] * ky
        d.ellipse((x - r0, y - r0, x + r0, y + r0), fill=(255, 255, 255, 70), outline=(255, 255, 255, 200), width=max(2, int(3 * kx)))
    # Ripples: each down expands and fades over 0.5 s.
    for ev in footage.touches:
        if ev["phase"] != "down":
            continue
        age = (ms - ev["t"]) / 500
        if 0 <= age <= 1:
            x, y = x0 + ev["x"] * kx, y0 + ev["y"] * ky
            r = r0 * (0.6 + 1.6 * ease(age))
            a = int(220 * (1 - age))
            d.ellipse((x - r, y - r, x + r, y + r), outline=(255, 255, 255, a), width=max(2, int(4 * kx)))
    canvas.alpha_composite(layer)


def toast(canvas, box, t, text_main, text_sub):
    """A notification card sliding into the top-right of a screen."""
    if t <= 0:
        return
    k = ease(t / 0.45)
    x0, y0, x1, y1 = box
    w = min(620, (x1 - x0) * 0.62)
    f1, f2 = font(BOLD, 30), font(REG, 24)
    h = 112
    x = x1 - w - 24 + (1 - k) * 80
    y = y0 + 24
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.rounded_rectangle((x, y, x + w, y + h), 20, fill=(24, 26, 42, int(240 * k)), outline=GOOD + (int(220 * k),), width=3)
    d.ellipse((x + 22, y + 30, x + 74, y + 82), fill=GOOD + (int(255 * k),))
    d.line((x + 36, y + 57, x + 46, y + 67, x + 62, y + 45), fill=(20, 24, 30, int(255 * k)), width=6)
    d.text((x + 94, y + 38), text_main, font=f1, fill=INK + (int(255 * k),), anchor="lm")
    sub_ = text_sub if f2.getlength(text_sub) <= w - 120 else text_sub[: int(len(text_sub) * (w - 140) / f2.getlength(text_sub))] + "…"
    d.text((x + 94, y + 76), sub_, font=f2, fill=DIM + (int(255 * k),), anchor="lm")
    canvas.alpha_composite(layer)


def cta_pill(canvas, t, y):
    """"Open source · coming soon" in a soft pill, fading in."""
    if t <= 0:
        return
    k = ease(t / 0.5)
    f = font(BOLD, 40 if L else 34)
    tw = f.getlength(CTA)
    x0 = W / 2 - tw / 2 - 40
    layer = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    d.rounded_rectangle((x0, y, x0 + tw + 80, y + 84), 42, fill=ACCENT + (int(36 * k),), outline=ACCENT + (int(210 * k),), width=3)
    d.text((W / 2, y + 42 + (1 - k) * 8), CTA, font=f, fill=INK + (int(255 * k),), anchor="mm")
    canvas.alpha_composite(layer)


def take(footage, start, seconds, beats, kind, zooms=(), focus_keys=(), ramp=None, toast_at=None):
    """One continuous take of `footage` from `start` (s), with headline beats
    (t, label, lines, accent, colour) and optional eased zooms
    (t_in, t_out, cx, cy, z) and, in portrait, a panning focus
    ((t, x) keyframes)."""
    @scene(seconds)
    def draw(c, t):
        k = ease(t / 0.6)
        src = ramp(t) if ramp else start + t
        shot = footage.at(src)
        z, cx, cy = 1.0, 0.5, 0.5
        for (zi, zo, zx, zy, zz) in zooms:
            w = ease_io((t - zi) / 0.8) * (1 - ease_io((t - zo) / 0.8))
            if w > 0:
                z, cx, cy = 1 + (zz - 1) * w, zx, zy
        if z > 1.001:
            shot = crop_zoom(shot, cx, cy, z)
        if kind == "laptop":
            caption_band(c, beats, t, "top")
            box = device_box("laptop", k)
            if L:
                laptop_frame(c, shot, box, alpha=k)
                if toast_at and t >= toast_at[0]:
                    toast(c, box, t - toast_at[0], toast_at[1], toast_at[2])
            else:
                fx = 0.5
                for (ft, fv) in focus_keys:
                    if t >= ft:
                        fx = fv
                # Ease between focus keyframes.
                prev = [f for f in focus_keys if f[0] <= t]
                nxt = [f for f in focus_keys if f[0] > t]
                if prev and nxt:
                    a, b2 = prev[-1], nxt[0]
                    fx = lerp(a[1], b2[1], ease_io((t - a[0]) / max(0.01, b2[0] - a[0])))
                bw, bh = int(box[2] - box[0]), int(box[3] - box[1])
                sw, sh = shot.size
                cw = sh * bw / bh
                x0 = min(max(fx * sw - cw / 2, 0), sw - cw)
                window_frame(c, shot.crop((int(x0), 0, int(x0 + cw), sh)), box, alpha=k)
                if toast_at and t >= toast_at[0]:
                    toast(c, box, t - toast_at[0], toast_at[1], toast_at[2])
        else:
            caption_band(c, beats, t, "side" if L else "top")
            box = device_box("phone", k)
            phone_frame(c, shot, box, alpha=k)
            if k > 0.95:
                fingers(c, footage, src, box)
    return draw


B, Y, G = ACCENT, AGENT, GOOD

# One tagline everywhere (video, README, repo): "Your Omarchy desktop, for
# you and your agents." The screens carry the features; text names them.

# 1. Laptop, one take: the desktop live in a tab, then giving a window to
#    another machine with the Spaces panel. Typing is sped up.
lt, lw = LAP.mark("Type and"), LAP.mark("Switch workspaces")
try:
    ls_ = LAP.mark("Drag a")
except KeyError:
    ls_ = None
L0 = 0.4
lap_end = (ls_ + 5.0) if ls_ is not None else lw + 2.6
lap_map, lap_inv = speed_map(L0, [(lt + 0.6, lw - 0.2, 1.8), (lw - 0.2, (ls_ or lw + 2.6) - 0.1, 2.2)])
lap_beats = [
    (lap_inv(L0 + 0.3), "REMOTE DESKTOP", ["Your Omarchy desktop,", "in any browser."], "browser", B),
    (lap_inv(lt + 0.8), "LIVE", ["Keyboard, mouse,", "60 fps."], "60", B),
]
if ls_ is not None:
    lap_beats.append((lap_inv(ls_ + 0.4), "SPACES", ["Drag it up.", "Drop it on another machine."], "drop", B))
take(LAP, L0, lap_inv(lap_end), lap_beats, "laptop",
     zooms=[(lap_inv(lt + 0.8), lap_inv(lw - 0.6), 0.75, 0.2, 2.0), (lap_inv(ls_ + 1.2), lap_inv(ls_ + 4.3), 0.6, 0.15, 1.9)] if (L and ls_) else [],
     focus_keys=[(0.0, 0.27), (lap_inv(lt + 0.4), 0.76), (lap_inv(lw), 0.5)] + ([(lap_inv(ls_ + 0.8), 0.52)] if ls_ else []),
     ramp=lap_map)


@scene(2.2)
def agents_card(c, t):
    s = 120 if L else 108
    lines = ["Now give your agents", "a desk."] if L else ["Now give", "your agents", "a desk."]
    y = H / 2 - s * 1.12 * len(lines) / 2
    kinetic(c, lines, t, W / 2, y, s, align="center", accent_word="agents")


# 2. Phone, one take: phone features quickly, then the agent story, then
#    every machine. Waiting (typing, page loads) is fast-forwarded.
pa, ps, pk, pw = PHONE.mark("Agents get"), PHONE.mark("It stops"), PHONE.mark("Take over"), PHONE.mark("Hand back")
try:
    pm = PHONE.mark("All your")
    phone_end = pm + 5.4
except KeyError:
    pm, phone_end = None, pw + 2.8
P0 = 0.5
ph_map, ph_inv = speed_map(P0, [
    (PHONE.mark("Type with") + 0.4, PHONE.mark("Swipe between") - 0.2, 2.5),  # typing on the phone
    (pa + 1.2, ps - 0.3, 1.8),                                                 # the agent fills the form
    (pk + 1.0, pw - 0.6, 1.6),                                                 # typing the code
])
beats = [
    (ph_inv(P0 + 0.2), "ON YOUR PHONE", ["Re-tiled for", "your screen."], "screen", B),
    (ph_inv(PHONE.mark("Long-press") + 0.6), "TOUCH", ["Drag to swap."], "swap", B),
    (ph_inv(PHONE.mark("Swipe between") + 0.3), "TOUCH", ["Swipe for", "workspaces."], "swipe", B),
    (ph_inv(pa + 0.3), "AGENTS", ["Its own Omarchy", "workspace."], "own", Y),
    (ph_inv(ps + 0.3), "AGENTS", ["It asks when", "it needs you."], "asks", Y),
    (ph_inv(pk + 0.3), "TAKE OVER", ["Step in.", "It waits."], "step", Y),
    (ph_inv(pw + 0.3), "HAND BACK", ["Done. It carries on."], "done", G),
]
if pm is not None:
    beats.append((ph_inv(pm + 0.5), "EVERY MACHINE", ["All your Omarchy", "machines, one tap."], "machines", B))
take(PHONE, P0, ph_inv(phone_end), beats, "phone", ramp=ph_map)


@scene(4.4)
def outro(c, t):
    s = 170 if L else 150
    k = ease(t / 0.5)
    y = H / 2 - s * 0.95
    text(c, "omaspace", (W / 2, y + (1 - k) * 40), font(BLACK, s), INK, alpha=k, anchor="ma", glow=ACCENT)
    tag = ["Your Omarchy desktop,", "for you and your agents."]
    for i, ln in enumerate(tag):
        text(c, ln, (W / 2, y + s * 1.25 + i * (56 if L else 50)), font(REG, 42 if L else 38), DIM if i == 0 else AGENT,
             alpha=ease((t - 0.5 - 0.3 * i) / 0.5), anchor="ma")
    k3 = ease((t - 0.9) / 0.5)
    text(c, "Over Tailscale  ·  only your devices  ·  no cloud", (W / 2, y + s * 1.25 + 130), font(BOLD, 30 if L else 28), DIM, alpha=k3, anchor="ma")
    k2 = ease((t - 1.2) / 0.5)
    f = font(MONO, 40 if L else 34)
    cmd = CTA
    tw = f.getlength(cmd)
    bx, by = W / 2 - tw / 2 - 34, y + s * 1.25 + 210
    box = Image.new("RGBA", c.size, (0, 0, 0, 0))
    ImageDraw.Draw(box).rounded_rectangle((bx, by, bx + tw + 68, by + 86), 18, fill=(26, 28, 46, int(235 * k2)), outline=ACCENT + (int(200 * k2),), width=2)
    c.alpha_composite(box)
    text(c, cmd, (W / 2, by + 43), f, ACCENT, alpha=k2, anchor="mm")


# ---- the 15-second cut ----------------------------------------------------------
# The hook, then the agent story on the phone (asks → take over → hand back),
# then the logo. Built from the same footage, fast.

if SHORT:
    scenes.clear()
    XF_SHORT = True

    @scene(1.5)
    def short_hook(c, t):
        s = 120 if L else 104
        lines = ["Your Omarchy desktop,"] if L else ["Your Omarchy", "desktop,"]
        y = H / 2 - s * (0.95 if L else 1.6)
        kinetic(c, lines, t * 1.6, W / 2, y, s, align="center", stagger=0.04)
        kinetic(c, ["for you and your agents."] if L else ["for you and", "your agents."], t * 1.6, W / 2, y + s * 1.12 * len(lines), s,
                align="center", accent_word="agents", lead=0.2, stagger=0.04)

    # Desktop: a beat of the live view and typing, then straight to the
    # Spaces panel: drag a window up, drop it on the other machine.
    sl0 = 0.4
    s_lap_map, s_lap_inv = speed_map(sl0, [
        (sl0 + 0.9, lt + 0.9, 12.0),        # skip to typing
        (lt + 0.9, lt + 3.4, 3.0),           # a flash of typing
        (lt + 3.4, ls_ + 0.6, 40.0),         # skip to the drag
        (ls_ + 0.6, ls_ + 4.4, 1.6),         # the drag and drop
    ])
    take(LAP, sl0, s_lap_inv(ls_ + 4.4), [
        (0.15, "REMOTE DESKTOP", ["In any browser."], "browser", B),
        (s_lap_inv(ls_ + 0.6), "SPACES", ["Drag it to another machine."] if L else ["Drag it to", "another machine."], "machine", B),
    ], "laptop",
        zooms=[(s_lap_inv(ls_ + 0.9), s_lap_inv(ls_ + 4.4) + 1, 0.6, 0.15, 1.9)] if L else [],
        focus_keys=[(0.0, 0.27), (s_lap_inv(lt + 0.9), 0.76), (s_lap_inv(ls_ + 0.7), 0.52)],
        ramp=s_lap_map)

    # Phone: swap and swipe, then the agent asks, you step in, done.
    ps0 = PHONE.mark("Long-press") + 0.3
    sw = PHONE.mark("Swipe between")
    s_ph_map, s_ph_inv = speed_map(ps0, [
        (ps0, ps0 + 2.4, 1.6),               # long-press, drag, swap
        (ps0 + 2.4, sw + 0.2, 60.0),         # skip typing
        (sw + 0.2, sw + 2.5, 1.8),           # one swipe
        (sw + 2.5, ps + 0.1, 40.0),          # skip to the agent asking
        (ps + 0.1, ps + 1.4, 1.3),           # the banner appears
        (ps + 1.4, pk + 0.2, 8.0),           # banner → take over
        (pk + 0.4, pw - 0.3, 4.5),           # type the code
    ])
    take(PHONE, ps0, s_ph_inv(pw + 0.8), [
        (0.0, "ON YOUR PHONE", ["Swap with a drag."], "drag", B),
        (s_ph_inv(sw + 0.2), "ON YOUR PHONE", ["Swipe workspaces."], "swipe", B),
        (s_ph_inv(ps + 0.1), "AGENTS", ["It asks.", "You step in."], "step", Y),
        (s_ph_inv(pw - 0.1), "AGENTS", ["Done."], "done", G),
    ], "phone", ramp=s_ph_map)

    @scene(2.1)
    def short_outro(c, t):
        s = 150 if L else 132
        k = ease(t / 0.35)
        y = H / 2 - s * 0.8
        text(c, "omaspace", (W / 2, y + (1 - k) * 30), font(BLACK, s), INK, alpha=k, anchor="ma", glow=ACCENT)
        text(c, "Your Omarchy desktop, for you and your agents.", (W / 2, y + s * 1.2), font(REG, 40 if L else 32), AGENT,
             alpha=ease((t - 0.25) / 0.35), anchor="ma")
        f = font(MONO, 38 if L else 32)
        text(c, CTA, (W / 2, y + s * 1.2 + 90), f, ACCENT, alpha=ease((t - 0.5) / 0.35), anchor="ma")


# ---- the teaser (~20 s) -------------------------------------------------------------
# One idea, four beats: any browser → any machine → your phone → your agents.

try:
    _peer = json.load(open(f"{SRC}/peer.json"))
except Exception:
    _peer = None

if TEASER:
    scenes.clear()

    @scene(2.4)
    def t_hook(c, t):
        s = 124 if L else 108
        lines = ["Your Omarchy desktop,"] if L else ["Your Omarchy", "desktop,"]
        y = H / 2 - s * (0.95 if L else 1.6)
        kinetic(c, lines, t, W / 2, y, s, align="center")
        kinetic(c, ["for you and your agents."] if L else ["for you and", "your agents."], t, W / 2, y + s * 1.12 * len(lines), s,
                align="center", accent_word="agents", lead=0.45)

    # Laptop: live in a tab, a flash of typing, then drag the browser up and
    # drop it on the other machine, where it arrives.
    arr = LAP.mark("On ")
    tl0 = 0.4
    t_lap_map, t_lap_inv = speed_map(tl0, [
        (tl0 + 1.8, lt + 0.9, 8.0),
        (lt + 0.9, lt + 4.2, 2.4),
        (lt + 4.2, ls_ + 0.6, 40.0),
        (ls_ + 0.6, ls_ + 4.4, 1.25),
        (ls_ + 4.4, arr, 5.0),
    ])
    peer_name = (_peer or {}).get("peer", "your other machine")
    take(LAP, tl0, t_lap_inv(arr + 2.4), [
        (0.15, "ANY BROWSER", ["Live in a tab."], "live", B),
        (t_lap_inv(ls_ + 0.6), "ANY MACHINE", ["Drag it to", "another machine."] if not L else ["Drag it to another machine."], "machine", B),
    ], "laptop",
        zooms=[(t_lap_inv(ls_ + 0.8), t_lap_inv(ls_ + 4.4), 0.6, 0.15, 1.9)] if L else [],
        focus_keys=[(0.0, 0.27), (t_lap_inv(lt + 0.9), 0.76), (t_lap_inv(ls_ + 0.6), 0.52)],
        ramp=t_lap_map,
        toast_at=(t_lap_inv(arr) - 0.1, f"Opened on {peer_name}", "Chromium · same tabs · workspace 4"))
    PUSH_AFTER.add(len(scenes) - 1)

    # Phone, part 1: an agent's own space. It works there, signs in, stops
    # at the 2FA code and asks; you take over, type it, hand back.
    a1 = pa + 0.3
    t_ag_map, t_ag_inv = speed_map(a1, [
        (a1, a1 + 2.4, 1.0),                 # its workspace, its browser
        (a1 + 2.4, ps - 0.2, 3.5),           # it fills in the sign-in
        (ps - 0.2, ps + 2.0, 1.1),           # the banner
        (ps + 2.0, pk + 0.2, 8.0),
        (pk + 0.2, pk + 1.4, 1.0),           # take over
        (pk + 1.4, pw - 0.3, 3.8),           # the code
    ])
    take(PHONE, a1, t_ag_inv(pw + 1.5), [
        (0.15, "YOUR AGENTS", ["Agents can have", "their own space."], "own", Y),
        (t_ag_inv(ps - 0.1), "YOUR AGENTS", ["It asks", "when it's stuck."], "asks", Y),
        (t_ag_inv(pk + 0.2), "TAKE OVER", ["Step in.", "It waits."], "step", Y),
        (t_ag_inv(pw - 0.1), "HAND BACK", ["Done.", "It carries on."], "done", G),
    ], "phone", ramp=t_ag_map)

    # Phone, part 2: your gestures. Swap, move to another workspace, swipe.
    q0 = PHONE.mark("Long-press") + 0.3
    qm = PHONE.mark("Long-press, let")
    qs = PHONE.mark("Swipe between")
    t_ph_map, t_ph_inv = speed_map(q0, [
        (q0, q0 + 2.6, 1.15),
        (q0 + 2.6, qm + 0.1, 12.0),
        (qm + 0.1, qm + 2.6, 1.1),
        (qm + 2.6, qs + 0.2, 40.0),
        (qs + 0.2, qs + 2.6, 1.2),
    ])
    take(PHONE, q0, t_ph_inv(qs + 2.6), [
        (0.0, "YOUR PHONE", ["Drag to swap."], "swap", B),
        (t_ph_inv(qm + 0.1), "YOUR PHONE", ["Move it anywhere."], "anywhere", B),
        (t_ph_inv(qs + 0.2), "YOUR PHONE", ["Swipe workspaces."], "swipe", B),
    ], "phone", ramp=t_ph_map)

    @scene(3.4)
    def t_outro(c, t):
        s = 156 if L else 136
        k = ease(t / 0.5)
        y = H / 2 - s * 0.85
        text(c, "omaspace", (W / 2, y + (1 - k) * 30), font(BLACK, s), INK, alpha=k, anchor="ma", glow=ACCENT)
        text(c, "Your Omarchy desktop, for you and your agents.", (W / 2, y + s * 1.2), font(REG, 40 if L else 32), AGENT,
             alpha=ease((t - 0.4) / 0.5), anchor="ma")
        cta_pill(c, t - 0.9, y + s * 1.2 + 100)


# ---- render --------------------------------------------------------------------

total = sum(s for s, _ in scenes)
XF = 0.2 if SHORT else 0.3 if TEASER else 0.35  # crossfade between scenes
ff = subprocess.Popen(
    ["ffmpeg", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
     "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", DST],
    stdin=subprocess.PIPE)


def render_scene(i, t):
    c = background(t_global).convert("RGBA")
    scenes[i][1](c, t)
    return c


n = int(total * FPS)
starts, acc = [], 0.0
for s, _ in scenes:
    starts.append(acc); acc += s
for fi in range(n):
    t_global = fi / FPS
    i = max(j for j in range(len(scenes)) if starts[j] <= t_global)
    t = t_global - starts[i]
    frame = render_scene(i, t)
    # crossfade into the next scene
    left = scenes[i][0] - t
    if left < XF and i + 1 < len(scenes):
        nxt = render_scene(i + 1, -(left))
        if i in PUSH_AFTER:
            # Sideways push: the next device slides in as this one leaves.
            pk_ = ease_io(1 - left / XF)
            off = int(pk_ * W)
            pushed = Image.new("RGBA", frame.size, (0, 0, 0, 255))
            pushed.paste(frame, (-off, 0))
            pushed.paste(nxt, (W - off, 0))
            frame = pushed
        else:
            frame = Image.blend(frame, nxt, 1 - left / XF)
    # fade from and to black
    if t_global < 0.4 or total - t_global < 0.6:
        a = min(t_global / 0.4, (total - t_global) / 0.6)
        frame = Image.blend(Image.new("RGBA", frame.size, (0, 0, 0, 255)), frame, max(0.0, min(1.0, a)))
    ff.stdin.write(frame.convert("RGB").tobytes())
    if fi % (FPS * 5) == 0:
        print(f"  {t_global:5.1f}s / {total:.1f}s", flush=True)
ff.stdin.close()
ff.wait()
print(DST, f"{total:.1f}s")
