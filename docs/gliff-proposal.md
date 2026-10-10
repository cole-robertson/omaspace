# Note to the gliff maintainers (draft, not sent)

To: Kevin McConnell, David Heinemeier Hansson
Subject: gliff in the browser and on phones: omaspace builds on gliff-server

Hi Kevin, David,

I've been building omaspace, a tool for Omarchy that puts your desktop in any
browser over Tailscale, with a phone-shaped screen, touch gestures, and
workspaces for AI agents (with "take over" when an agent needs a person). When
gliff landed it was clearly the right engine, so I rebuilt omaspace's live view
on top of it rather than keep my own recorder:

- omaspace runs `gliff-server --stdio` per viewer, exactly as your client does
  over ssh, and bridges gliff's protocol (via `gliff-proto`, pinned to v0.3.0)
  to a WebSocket in the browser.
- Phones get gliff's `--headless` screen sized to the phone.
- Laptop browsers get full 4:4:4: both 4:2:0 streams decode in WebCodecs and a
  WebGL2 shader recombines them using gliff-proto's AVC444 layout. Coloured
  text comes through as sharp as in your client.
- Clipboard text goes both ways over gliff's clipboard messages, and a dropped
  frame sends `RequestKeyframe`.

Nothing in gliff had to change. Three things would make it smoother, and I'm
happy to send PRs if you want them:

1. **A stable library API** for `gliff-proto` (or publishing it), so other
   front ends can depend on it without pinning a git tag.
2. **A "browser" feature flag in Hello**, so gliff-server could favour browser
   decoders: no 4:4:4 when the client says it can't recombine, and periodic
   keyframes for clients that don't request them.
3. **The headless scale rounding** in `set_monitor_mode`: a phone at 3x with a
   size the scale doesn't divide gets Hyprland's "Invalid scale, failed to find
   a clean divisor" banner. omaspace now rounds the size first; gliff could too.

Bigger question: would a browser/phone viewer belong in gliff itself? If so,
omaspace could become the layer on top (agents, take over, moving workspaces
between machines, file sync), with the viewer upstream. Either way, thanks for
gliff; it made the live view much better.

Cole
