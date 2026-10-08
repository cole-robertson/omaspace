#!/usr/bin/env python3
"""The social card (X/Twitter, GitHub, Open Graph): 1200x675 (and a 2x copy),
from the same footage and drawing helpers as the launch video.

    python3 demo-card.py /tmp/demo /tmp/omaspace-card.png
    python3 demo-card.py /tmp/demo /tmp/omaspace-card-vertical.png --vertical
                                          1080x1920, the poster frame for vertical video
"""
import sys

SRC, DST = sys.argv[1], sys.argv[2]
VERTICAL = "--vertical" in sys.argv
sys.argv = ["demo-launch.py", SRC, "/dev/null"]
src = open(__file__.replace("demo-card.py", "demo-launch.py")).read().split("# ---- render ----")[0]
g = {"__name__": "card"}
exec(compile(src, "demo-launch.py", "exec"), g)
Image, ImageDraw = g["Image"], g["ImageDraw"]
font, text, ease = g["font"], g["text"], g["ease"]
BLACK, BOLD, REG, MONO = g["BLACK"], g["BOLD"], g["REG"], g["MONO"]
INK, DIM, ACCENT, AGENT = g["INK"], g["DIM"], g["ACCENT"], g["AGENT"]
LAP, PHONE = g["LAP"], g["PHONE"]

if VERTICAL:
    W, H = 2160, 3840  # drawn at 2x, saved at both sizes
    g["W"], g["H"] = W, H
    c = g["background"](8.0).convert("RGBA")
    lap_shot = LAP.at(LAP.mark("Type and") + 3.0)
    phone_shot = PHONE.at(PHONE.mark("Take over") + 1.2)
    # Words on top, the laptop under them, the phone overlapping its corner.
    x = 150
    text(c, "omaspace", (x, 260), font(BLACK, 190), INK, glow=ACCENT)
    for i, (ln, col) in enumerate([("Your Omarchy desktop,", INK), ("for you and", INK), ("your agents.", ACCENT)]):
        text(c, ln, (x, 560 + i * 130), font(BLACK, 112), col)
    lw = 1860; lh = int(lw * 900 / 1440); lx, ly = 150, 1080
    g["laptop_frame"](c, lap_shot, (lx, ly, lx + lw, ly + lh))
    ph = 1700; pw = int(ph / g["PHONE_ASPECT"]); px, py = W - pw - 150, 1700
    g["phone_frame"](c, phone_shot, (px, py, px + pw, py + ph))
    for i, ln in enumerate(["Remote desktop", "in any browser.", "", "Agents in their", "own workspace.", "", "Step in from", "your phone."]):
        text(c, ln, (x, 2400 + i * 74), font(REG, 60), DIM)
    f = font(BOLD, 64)
    tag = "Open source · coming soon"
    tw = f.getlength(tag)
    d = ImageDraw.Draw(c)
    y = 3520
    d.rounded_rectangle((W / 2 - tw / 2 - 60, y, W / 2 + tw / 2 + 60, y + 124), 62, fill=ACCENT + (36,), outline=ACCENT + (210,), width=4)
    text(c, tag, (W / 2, y + 62), f, INK, anchor="mm")
    img = c.convert("RGB")
    img.save(DST.replace(".png", "@2x.png"))
    img.resize((1080, 1920), Image.LANCZOS).save(DST)
    print(DST)
    sys.exit(0)

W, H = 2400, 1350  # drawn at 2x, saved at both sizes
g["W"], g["H"] = W, H
c = g["background"](8.0).convert("RGBA")

# The laptop on the right, the phone overlapping it in front: the two ways in.
lap_shot = LAP.at(LAP.mark("Type and") + 3.0)
phone_shot = PHONE.at(PHONE.mark("Take over") + 1.2)  # "You're in control" on the bank page
lw = 1180; lh = int(lw * 900 / 1440); lx, ly = 1180, 330
g["laptop_frame"](c, lap_shot, (lx, ly, lx + lw, ly + lh))
ph = 1010; pw = int(ph / g["PHONE_ASPECT"]); px, py = 1080, 230
g["phone_frame"](c, phone_shot, (px, py, px + pw, py + ph))

# Words on the left.
x = 110
text(c, "omaspace", (x, 200), font(BLACK, 140), INK, glow=ACCENT)
for i, (ln, col) in enumerate([("Your Omarchy", INK), ("desktop, for you", INK), ("and your agents.", ACCENT)]):
    text(c, ln, (x, 410 + i * 92), font(BLACK, 80), col)
for i, ln in enumerate(["Remote desktop in any browser.", "Agents in their own workspace.", "Step in from your phone."]):
    text(c, ln, (x, 750 + i * 60), font(REG, 42), DIM)
f = font(BOLD, 42)
tag = "Open source · coming soon"
tw = f.getlength(tag)
d = ImageDraw.Draw(c)
d.rounded_rectangle((x, 1010, x + tw + 80, 1094), 42, fill=ACCENT + (36,), outline=ACCENT + (210,), width=3)
text(c, tag, (x + 40, 1052), f, INK, anchor="lm")

img = c.convert("RGB")
img.save(DST.replace(".png", "@2x.png"))
img.resize((1200, 675), Image.LANCZOS).save(DST)
print(DST)
