#!/usr/bin/env python3
"""An original soundtrack for the launch videos, synthesized with numpy (no
samples, no licence questions). 120 BPM in A minor: a pumping supersaw pad, a
sub and an offbeat saw bass, a plucked arp with a ping-pong delay, kick, clap
and hats; a riser into every scene cut and an impact on it; a bell motif on
the logo. Mastered to -14 LUFS with ffmpeg's loudnorm.

    python3 demo-music.py OUT.wav SECONDS CUT1 CUT2 ...

INTRO (env, seconds) is where the beat drops; the last cut is the logo.
"""
import json, os, subprocess, sys, tempfile, wave
import numpy as np

OUT, SECONDS = sys.argv[1], float(sys.argv[2])
CUTS = [float(c) for c in sys.argv[3:]]
INTRO = float(os.environ.get("INTRO", "3.2"))
LOGO = CUTS[-1] if CUTS else SECONDS - 3.4
INNER = [c for c in CUTS[:-1] if c > INTRO + 0.5]  # scene cuts inside the beat
SR = 44100
BPM = 120
BEAT = 60 / BPM
BAR = 4 * BEAT
N = int((SECONDS + 4) * SR)  # room for tails; trimmed at the end
rng = np.random.default_rng(7)

# Am9 – Fmaj9 – Cadd9 – G6, a bar each: (bass root, pad voicing).
CHORDS = [
    (45, [57, 60, 64, 67, 71]),
    (41, [53, 57, 60, 64, 67]),
    (48, [55, 60, 62, 64, 67]),
    (43, [55, 59, 62, 64, 67]),
]


def hz(m):
    return 440.0 * 2 ** ((m - 69) / 12)


def bus():
    return np.zeros((2, N))


def place(buf, t, sig, gain=1.0, pan=0.0):
    """Add a mono or stereo signal into buf at time t (equal-power pan)."""
    i = int(round(t * SR))
    if i >= N:
        return
    if sig.ndim == 1:
        a = (pan + 1) * np.pi / 4
        sig = np.stack([sig * np.cos(a), sig * np.sin(a)]) * np.sqrt(2)
    if i < 0:
        sig, i = sig[:, -i:], 0
    n = min(sig.shape[1], N - i)
    buf[:, i:i + n] += sig[:, :n] * gain


def filt(x, lo=None, hi=None, order=2):
    """Zero-phase Butterworth-shaped filter in the frequency domain."""
    X = np.fft.rfft(x, axis=-1)
    f = np.fft.rfftfreq(x.shape[-1], 1 / SR)
    f[0] = 1e-3
    H = np.ones_like(f)
    if hi:
        H /= np.sqrt(1 + (f / hi) ** (2 * order))
    if lo:
        H /= np.sqrt(1 + (lo / f) ** (2 * order))
    return np.fft.irfft(X * H, n=x.shape[-1], axis=-1)


def shift(x, sec):
    s = int(sec * SR)
    y = np.zeros_like(x)
    y[..., s:] = x[..., :-s]
    return y


def ts(sec):
    return np.arange(int(sec * SR)) / SR


def fade(sig, a=0.004, r=0.02):
    n = len(sig)
    e = np.ones(n)
    na, nr = max(1, int(a * SR)), max(1, int(r * SR))
    e[:na] = np.linspace(0, 1, na)
    e[-nr:] *= np.linspace(1, 0, nr)
    return sig * e


# ---- the grid --------------------------------------------------------------
# Beat k sits at INTRO + k*BEAT, so the drop lands on a downbeat.
k0 = int(np.floor(-INTRO / BEAT)) - 4
k1 = int(np.ceil((LOGO - INTRO) / BEAT))
beats = [(k, INTRO + k * BEAT) for k in range(k0, k1) if INTRO + k * BEAT >= -BAR]
in_beat = lambda t: INTRO <= t < LOGO - 0.05
before_cut = lambda t: any(c - 0.55 < t < c - 0.02 for c in INNER)
second = CUTS[1] if len(CUTS) > 2 else INTRO + 2 * BAR   # the arp comes in
third = CUTS[2] if len(CUTS) > 3 else INTRO + 6 * BAR    # open hats too


def chord_at(k):
    return CHORDS[(k // 4) % 4]


# ---- drums -----------------------------------------------------------------
def kick():
    t = ts(0.45)
    f = 45 + 125 * np.exp(-t * 32)
    body = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 6.5)
    click = filt(rng.standard_normal(len(t)), lo=1500) * np.exp(-t * 350) * 0.25
    return np.tanh(1.8 * (body + click))


def clap():
    t = ts(0.4)
    e = np.zeros_like(t)
    for d, dec in ((0, 70), (0.010, 70), (0.021, 13)):
        e += (t >= d) * np.exp(-np.clip(t - d, 0, None) * dec)
    return rng.standard_normal(len(t)) * e


def hat(dur, dec):
    t = ts(dur)
    return rng.standard_normal(len(t)) * np.exp(-t * dec)


drums, claps, hats = bus(), bus(), bus()
duck = np.ones(N)
K = kick()
dk = 1 - 0.65 * np.exp(-ts(0.5) / 0.11) * np.minimum(1, ts(0.5) / 0.004)
for k, t in beats:
    if not in_beat(t) or before_cut(t):
        continue
    place(drums, t, K, 0.95)
    i = int(t * SR)
    duck[i:i + len(dk)] = np.minimum(duck[i:i + len(dk)], dk[:N - i])
    if k % 4 in (1, 3):
        place(claps, t, clap(), 0.5, pan=0.05)
    for s, v in enumerate((0.55, 0.25, 0.8, 0.3)):
        th = t + s * BEAT / 4
        if in_beat(th) and not before_cut(th):
            place(hats, th, hat(0.05, 95), v * 0.22, pan=0.25)
    if t >= third:
        place(hats, t + BEAT / 2, hat(0.28, 11), 0.13, pan=-0.2)
claps = filt(claps, lo=900, hi=7000)
hats = filt(hats, lo=7000, order=3)

# ---- bass ------------------------------------------------------------------
sub, saws = np.zeros(N), np.zeros(N)
for k, t in beats:
    if k % 4 or not (INTRO - 0.01 <= t < LOGO):
        continue
    root = chord_at(k)[0]
    seg = np.sin(2 * np.pi * hz(root - 12) * ts(min(BAR, LOGO - t)))
    place_t = int(t * SR)
    s = fade(seg, 0.005, 0.02)
    sub[place_t:place_t + len(s)] += s[:N - place_t]
for k, t in beats:
    to = t + BEAT / 2
    if not (INTRO <= to < LOGO - 0.1):
        continue
    root = chord_at(k)[0] + (12 if k % 4 == 3 else 0)
    tt = ts(0.21)
    sw = 2 * ((hz(root) * tt) % 1) - 1
    i = int(to * SR)
    s = fade(sw * np.exp(-tt * 9), 0.003, 0.03)
    saws[i:i + len(s)] += s[:N - i]
bass = np.stack([sub * 0.42 + filt(saws, hi=850) * 0.2] * 2)

# ---- pad: five detuned saws per note, spread across the stereo field --------
pad = bus()
DET = [(-14, -0.7), (-7, 0.35), (0, 0.0), (7, -0.35), (14, 0.7)]
for k, t in beats:
    if k % 4 or t >= LOGO:
        continue
    dur = BAR + 0.25
    tt = ts(dur)
    env = np.minimum(1, tt / 0.06) * np.minimum(1, (dur - tt) / 0.25)
    for m in chord_at(k)[1]:
        for cents, p in DET:
            f = hz(m) * 2 ** (cents / 1200)
            ph = rng.random()
            place(pad, t, (2 * ((f * tt + ph) % 1) - 1) * env, 0.022, pan=p)
# The hook opens the filter into the drop; the beat runs bright.
w = np.clip((np.arange(N) / SR - (INTRO - 2.2)) / 2.2, 0, 1) ** 2
pad = filt(pad, hi=420) * (1 - w) + filt(pad, hi=3200) * w
pad *= np.clip((LOGO - np.arange(N) / SR) / 0.06, 0, 1)  # out on the logo

# ---- arp: 16ths over the chord, plucked, with a dotted-8th ping-pong --------
_pl = {}


def pluck(m):
    if m not in _pl:
        f = hz(m)
        t = ts(0.26)
        fc = 700 + 4200 * np.exp(-t * 22)
        out = np.zeros_like(t)
        for h in range(1, int(9000 / f) + 1):
            out += (1 / h) / np.sqrt(1 + (h * f / fc) ** 4) * np.sin(2 * np.pi * h * f * t)
        _pl[m] = fade(out * np.exp(-t * 11), 0.002, 0.03)
    return _pl[m]


arp = np.zeros(N)
ORDER = [0, 2, 4, 5, 3, 1, 4, 2]
for k, t in beats:
    v = chord_at(k)[1]
    tones = sorted(v) + [v[0] + 12, v[2] + 12]
    for s in range(4):
        ta = t + s * BEAT / 4
        if not (second - 0.05 <= ta < LOGO - 0.05):
            continue
        rise = min(1, (ta - second + BAR) / BAR)  # swells in over a bar
        m = tones[ORDER[(k * 4 + s) % 8]] + 12
        i = int(ta * SR)
        p = pluck(m) * (1.0 if s == 0 else 0.7) * rise
        arp[i:i + len(p)] += p[:N - i]
D = 0.75 * BEAT
echo = filt(arp, lo=300, hi=3500)
arps = np.stack([arp + 0.42 * shift(echo, D) + 0.18 * shift(echo, 3 * D),
                 arp + 0.28 * shift(echo, 2 * D) + 0.12 * shift(echo, 4 * D)]) * 0.16

# ---- transitions: a riser into each cut, an impact on it -------------------
fx = bus()


def riser(length):
    t = ts(length)
    x = t / length
    n = rng.standard_normal(len(t))
    bands = [filt(n, lo=200, hi=900), filt(n, lo=900, hi=3500), filt(n, lo=3500, hi=12000)]
    wts = [np.clip(1 - 2 * x, 0, 1), 1 - np.abs(2 * x - 1), np.clip(2 * x - 1, 0, 1)]
    noise = sum(b * w_ for b, w_ in zip(bands, wts))
    tone = np.sin(2 * np.pi * np.cumsum(250 * 2 ** (2.5 * x)) / SR) * 0.15
    return fade((noise + tone) * x ** 2.2, 0.01, 0.01)


def boom(big=False):
    t = ts(2.2 if big else 1.4)
    f = 38 + 30 * np.exp(-t * 6)
    s = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * (1.6 if big else 2.6))
    hit = filt(rng.standard_normal(len(t)), lo=150, hi=4000) * np.exp(-t * 9) * 0.35
    return np.tanh(1.5 * (s + hit))


place(fx, 0.15, riser(INTRO - 0.15), 0.32)
place(fx, INTRO, boom(), 0.55)
for c in INNER:
    place(fx, c - 1.3, riser(1.3), 0.26)
    place(fx, c, boom(), 0.45)
if CUTS:
    place(fx, LOGO - 1.5, riser(1.5), 0.3)
    place(fx, LOGO, boom(big=True), 0.6)

# ---- the logo: a held Am9 and a bell motif ---------------------------------
def bell(m, dur=2.8):
    t = ts(dur)
    f = hz(m)
    idx = 2.6 * np.exp(-t * 5)
    return fade(np.sin(2 * np.pi * f * t + idx * np.sin(2 * np.pi * f * 3.5 * t)) * np.exp(-t * 2.0), 0.002, 0.2)


end = bus()
tail = SECONDS - LOGO + 2
tt = ts(tail)
for m in CHORDS[0][1]:
    for cents, p in DET:
        f = hz(m) * 2 ** (cents / 1200)
        sig = (2 * ((f * tt + rng.random()) % 1) - 1) * np.exp(-tt * 0.7) * np.minimum(1, tt / 0.02)
        place(end, LOGO, sig, 0.02, pan=p)
end = filt(end, hi=2600)
bells = bus()
for i, (m, p) in enumerate(((76, -0.3), (81, 0.3), (83, -0.15), (88, 0.15))):
    place(bells, LOGO + 0.05 + i * BEAT / 2, bell(m), 0.22, pan=p)
for m in (81, 88):  # as the "coming soon" pill lands
    place(bells, LOGO + 0.95, bell(m, 2.4), 0.14, pan=0.0)

# ---- reverb: a synthetic stereo hall, by FFT convolution -------------------
def hall(sec=2.4):
    t = ts(sec)
    ir = rng.standard_normal((2, len(t))) * np.exp(-t * 2.8)
    ir = filt(ir, lo=200, hi=6500)
    ir[:, :int(0.02 * SR)] = 0  # pre-delay
    return ir / np.sqrt((ir ** 2).sum(axis=1, keepdims=True))


send = claps * 0.35 + arps * 0.3 + pad * 0.12 + fx * 0.4 + bells * 0.6 + end * 0.3
ir = hall()
size = 1 << int(np.ceil(np.log2(N + ir.shape[1])))
verb = np.fft.irfft(np.fft.rfft(send, size) * np.fft.rfft(ir, size), size)[:, :N] * 0.5

# ---- mix and master --------------------------------------------------------
pump = np.stack([duck, duck])
mix = drums + claps + hats + fx + bells + end + (bass + pad + arps + verb * 0.6) * pump + verb * 0.4
mix = filt(mix, lo=28)
mix = mix[:, :int(SECONDS * SR)]
t = np.arange(mix.shape[1]) / SR
mix *= np.minimum(1, t / 0.3) * np.clip((SECONDS - t) / 1.2, 0, 1)
mix /= np.abs(mix).max() + 1e-9
mix = np.tanh(1.2 * mix) / np.tanh(1.2)

for name, x in (("drums", drums), ("bass", bass), ("pad", pad), ("arp", arps), ("fx", fx), ("verb", verb)):
    print(f"  {name:6s} {20 * np.log10(np.sqrt((x ** 2).mean()) + 1e-12):6.1f} dB rms", file=sys.stderr)

tmp = tempfile.mktemp(suffix=".wav")
with wave.open(tmp, "wb") as wv:
    wv.setnchannels(2); wv.setsampwidth(2); wv.setframerate(SR)
    wv.writeframes((mix.T * 0.95 * 32767).astype("<i2").tobytes())

# Two-pass loudnorm: measure, then normalise linearly to -14 LUFS / -1.5 dBTP.
LN = "loudnorm=I=-14:TP=-1.5:LRA=11"
r = subprocess.run(["ffmpeg", "-hide_banner", "-i", tmp, "-af", LN + ":print_format=json", "-f", "null", "-"],
                   capture_output=True, text=True)
m = json.loads(r.stderr[r.stderr.rindex("{"):r.stderr.rindex("}") + 1])
af = (f"{LN}:measured_I={m['input_i']}:measured_TP={m['input_tp']}:measured_LRA={m['input_lra']}"
      f":measured_thresh={m['input_thresh']}:offset={m['target_offset']}:linear=true")
subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", tmp, "-af", af, "-ar", str(SR), OUT], check=True)
os.unlink(tmp)
print(OUT)
