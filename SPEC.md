# omaspace

Move an Omarchy workspace between your own machines (Omarchy desktop, laptop,
plain Linux server) over Tailscale. MIT. Omarchy/Hyprland first, Linux only.

Written after reading cua Spaces (teleport, cua-spacesd) as a reference. No cua
code is copied; omaspace is a cut-down, Omarchy-specific reimplementation of the
idea. Agent control of the desktop is left to cua-driver (MIT), which omaspace
points agents at instead of reimplementing.

## What moves

A **snapshot** is the restorable state of one Hyprland workspace (or all of them):

| Kind | Captured from | Restored as |
|---|---|---|
| Browser window | Chromium `Sessions/Session_*` (SNSS), matched to Hyprland windows | `chromium --new-window <urls…>` in the same workspace |
| Terminal | Hyprland client pid → shell child's `/proc/<pid>/cwd`; tmux client → session name | Omarchy terminal in that directory; `tmux new -A -s <name>` |
| Editor | Hyprland client (Neovim inside a terminal: `/proc` cmdline file args) | terminal running `nvim <files>` in the same directory |
| Other app | Hyprland `class` + `/proc/<pid>/cmdline` (desktop-file allowlist below) | same command |
| Layout | Hyprland workspace id, floating, size/position, fullscreen | `hl.dsp.window.*` rules applied when the window maps |

### Browser profile data (follows cua Spaces' defaults)

A browser window can carry its profile data, chosen per item exactly like cua
Spaces' Chrome teleport: everything that is not credential-shaped moves by
default; the three credential-shaped items are withheld unless the user allows
them.

| Item | cua default | omaspace |
|---|---|---|
| Open tabs + `Sessions/` (window layout) | on | on |
| Preferences, Bookmarks, Web Data | on | on |
| Local Storage, Session Storage (site data) | on | on |
| **Cookies** (sign-ins) | **off** (sensitive) | off unless allowed |
| **Login Data** (saved passwords) | **off** (sensitive) | off unless allowed |
| **History** | **off** (sensitive, large) | off unless allowed |

Allowing sensitive items: interactively (`--allow-sensitive`, or the Omarchy
menu asks and names the browser), or ahead of time for unattended use in
`~/.config/omaspace/policy.json` → `{"allow_sensitive": ["cookies", …]}`, the
counterpart of cua's `~/.cua/spaces-teleport-policy.json`.

Cookies and saved passwords are never copied as raw files: they are decrypted
on the sender (Linux `v10` key "peanuts" or the `v11` key from libsecret,
`application=chrome` then `chromium`) and re-encrypted with the receiver's own
key, with Chrome 130+'s host-key digest when the receiving database uses it.

Like cua, the receiving browser's files are written while that profile is not
running: the receiver closes the running browser for that profile, writes,
and relaunches it with the restored tabs. Cookies and local storage are merged
into the existing profile (`INSERT OR REPLACE` per cookie, per storage key), so
the receiver's other sign-ins stay. Bookmarks/Preferences/Web Data/Sessions
are only copied into a profile that doesn't have them yet.

Never captured: keyring secrets, environment variables, shell history.

Apps are only relaunched from a fixed set of known launchers (Chromium,
Omarchy terminal/foot/alacritty/ghostty/kitty, nvim, code/zed via their
binaries, and anything with an installed `.desktop` file whose `Exec` matches
the captured binary). A snapshot cannot make the destination run an arbitrary
command line.

## Snapshot format (`omaspace.snapshot.v1`, JSON)

```json
{
  "format": "omaspace.snapshot.v1",
  "id": "ws-…", "source": "bravo", "taken_at": "RFC3339",
  "workspaces": [{
    "id": 3, "name": "3",
    "windows": [{
      "kind": "browser|terminal|editor|app",
      "class": "chromium", "title": "…",
      "floating": false, "at": [x, y], "size": [w, h], "fullscreen": 0,
      "browser":  { "profile": "Default", "urls": ["…"], "active": 0 },
      "terminal": { "cwd": "/home/u/src/x", "tmux_session": "work" },
      "editor":   { "cwd": "…", "files": ["…"] },
      "app":      { "desktop_id": "obsidian.desktop" }
    }]
  }]
}
```

Paths under the source user's `$HOME` are stored relative (`~/…`) and rebased
onto the destination's home. A directory missing on the destination falls
back to `~` and is reported, never silently.

## Peers and trust

- Each machine runs `omaspace serve` as a user service listening on its own
  **Tailscale IPv4 address** (`tailscale ip -4`), port 7787, plain HTTP inside
  the WireGuard tunnel. It never binds a LAN or public address.
- Every request is identified with tailscaled's LocalAPI
  (`/run/tailscale/tailscaled.sock`, `GET /localapi/v0/whois?addr=<ip:port>`).
  Trust rule, checked per request:
  - this machine is **user-owned** → the caller must be owned by the same
    Tailscale user (`UserProfile.ID`);
  - this machine is **tagged** (e.g. `tag:desktop`) → the caller must carry at least
    one of this machine's tags.
  Anything else, or a whois failure, gets 403. No tokens to copy around.
  (Verified 2026-10-02: Tailscale Serve adds no identity headers for tagged
  devices, so whois on the raw peer address is the reliable identity.)
- The machine itself is never a trusted caller (whois identifies a local
  process's connection to the machine's own address as this node), and the
  daemon only answers requests whose `Host` is its tailnet IP and that carry
  no `Origin` header (no DNS rebinding, no cross-site forms).
- Peers are discovered from `tailscale status --json`: online peers that pass
  the same trust rule and answer `GET /v1/hello` on port 7787.

## API (JSON over HTTP on the tailnet address, port 7787)

| Method | Path | |
|---|---|---|
| GET | `/v1/hello` | name, version, has_desktop, workspaces |
| GET | `/v1/snapshot?workspace=N\|all` | capture now and return a snapshot |
| POST | `/v1/restore` | body: snapshot; restore onto this desktop; returns per-window results |
| GET | `/v1/stash` | list snapshots stored here (server/hub role) |
| PUT | `/v1/stash/<id>` | store a snapshot |
| GET | `/v1/stash/<id>` | fetch a stored snapshot |

Restore returns, per window: `restored`, `skipped(reason)`, or `failed(error)`.
Counts are reported to the user; a partial restore never reports success.

## Roles

- **Desktop** (Omarchy/Hyprland session present): capture, restore, stash.
- **Hub** (headless Linux server): stash only. Desktops push their latest
  snapshot there on `send`, and a laptop can `pull` it later.

## CLI (`omaspace`)

```
omaspace peers                         # machines on your tailnet running omaspace
omaspace snapshot [--workspace N|all] [--out file]
omaspace send <peer> [--workspace N|all]   # capture here, restore there
omaspace pull <peer> [--workspace N|all]   # capture there, restore here
omaspace stash <hub> [--workspace N|all]   # store on a hub
omaspace restore <file|hub:id>
omaspace serve                         # the daemon (systemd user unit runs this)
omaspace setup                         # unit + tailscale serve + Omarchy bindings
```

## Omarchy integration

- Keybindings (in `~/.config/hypr/bindings.lua` via `o.bind`):
  `SUPER+CTRL+SHIFT+O` the Spaces panel (give, take back, watch),
  `SUPER+CTRL+ALT+O` give the current workspace (peer picker).
- Menus via `omarchy-menu-select`, notifications via `notify-send`.

### Spaces panel (`omaspace.spaces`, an Omarchy shell overlay plugin)

The Omarchy take on cua Spaces' "drag a window to the top, pick a Space"
(cua does this on macOS only: an event tap watches window drags and a notch
panel opens; on Wayland a client can't see other apps' drags, so cua has no
Linux version). Hyprland can: omaspace binds `SUPER + mouse:272` press and
release as `non_consuming`, so Omarchy's own SUPER-drag still moves the window.

- **Drag to the top.** While SUPER-dragging, a watcher polls the cursor (60 ms,
  only during the drag). When the pointer is in the top 24 px, the panel slides
  down from the top edge: one tile per machine (this one included), each with
  its workspaces 1–9 and their window counts. Over a machine's tile, dropping
  gives the dragged window to that machine, on the same workspace number; over
  a workspace cell, onto that workspace. On this machine's tile, onto that
  workspace here. Dropping anywhere else, or pressing Esc, gives nothing.
- **Keyboard.** `SUPER+CTRL+SHIFT+O` opens the same panel in place of a
  menu. ←/→ picks a machine, ↑/↓ or 1–9 picks a workspace; `Enter` gives the
  current workspace (or the picked one on this machine) to the selected
  machine, `T` takes the selected workspace back from it, `W` watches it
  (live view), `F` sends files to it, `S` syncs a folder with it, `Esc` closes.
- **Data.** `omaspace spaces` prints this machine and every desktop peer with
  workspaces, windows and the active workspace (`GET /v1/workspaces` per
  peer). A peer that doesn't answer stays in the list with its error.
- **Actions** go through `omarchy-omaspace` (sign-in prompt, notifications),
  so the panel, the keybinding and the menu do the same thing.
  `omaspace send <peer> --window <address>` moves one window.
- **Tests.** `debugState()` (via `omarchy-shell shell call omaspace.spaces
  debugState ''`) returns the panel's state as JSON for the e2e suite.
- Restored windows are placed with Hyprland window rules keyed on a one-shot
  token so they land in the original workspace even if the user switches.

## Agent spaces (agents next to you, on your own machine)

An agent works on its own Omarchy workspace on the same machine as you, never
touching your keyboard, pointer or focus. You swipe to its workspace to watch.

- `claim_space{agent, task}` gives it the highest free workspace in 5–9 (not
  the one you're on, no windows on it); `set_status`, `ask_for_help`,
  `get_space`, `release_space`. State is `~/.local/state/omaspace/agents.json`
  (flock'd), shared by the daemon, the live view and each agent's MCP server.
- **Its browser** (`open_browser`): a Chromium in a throwaway profile under
  `~/.local/state/omaspace/agent-browsers/<id>`, seeded with *all* your cookies
  and local storage (`profile::capture_all` → `apply_all`; passwords and
  history are not copied), class `omaspace-agent`, opened `silent` on the
  agent's workspace. DevTools is never on a TCP port (any local user could
  read the copied cookies over loopback): Chromium runs with
  `--remote-debugging-pipe` under `omaspace cdp-broker`, which relays it on
  `$XDG_RUNTIME_DIR/omaspace-cdp/<id>.sock` (0600, in a 0700 directory). Driven over CDP (`cdp.rs`:
  navigate, click by selector, type, key, read, eval, screenshot, cookies), so
  no input events touch your session. A Hyprland rule
  (`suppress_event = "activate activatefocus"` for that class, installed by
  setup) stops Omarchy's `focus_on_activate` from jumping to it on a click.
- **Hand a browser over** (long-press a browser window → "Hand to <agent>"):
  its tabs open in a new agent browser with your sign-ins; your window closes.
  **Take back** (agent sheet, or `take_browser_back`): the agent browser is
  closed through CDP (`Browser.close`, then it exits on its own so Chromium
  flushes its lazily-written cookie store), its cookies and storage merge into
  your profile (your Chromium restarts if running; its exit is marked clean so
  it doesn't restore the old session), its tabs reopen on your workspace, and
  its profile is deleted.
- **Release** stops its browsers and deletes their profiles.
- **Live view**: agent workspaces carry the agent's initial in the strip
  (yellow; red and pulsing when it needs you); on its workspace a bar shows
  its name and status; tap for its sheet (Watch, Done helping, Take back each
  browser). Help requests raise the banner and `notify-send`.
- **Its terminals** (`open_terminal`, `terminal_run/type/key/read`): the
  Omarchy terminal (foot, class `omaspace-agent-term`) attached to a tmux
  session on a private server (`tmux -L omaspace-agents`). The agent types
  with `send-keys` and reads with `capture-pane`; nothing reaches your input.
- **Any other app** (`open_app`, `app_read/click/set_value/type/key`,
  `close_window`): through cua-driver, from the `omaspace-driver` package.
  Reading (accessibility tree + window screenshot) and clicking/setting fields
  by accessibility work for GTK/Qt apps in the background with no plugin. Raw
  typing goes through cua's Hyprland plugin on its own seat, for apps listed
  in `~/.config/cua-driver/qualified-apps`, and only after a Hyprland start
  with the plugin loaded. Every call is checked: the window must be on the
  agent's workspace. Single-instance apps (Nautilus) that open their window on
  your workspace are moved to the agent's. Each agent uses its own driver
  session, so tokens from `app_read` stay valid for its next click.

### omaspace-driver (pkg/omaspace-driver)

cua-driver 0.32.0 and cua's Hyprland plugin (both MIT) from trycua/cua, with
two Omarchy patches: a per-user allowlist (`~/.config/cua-driver/qualified-apps`)
and keeping foreground typing working under Omarchy's key remaps. Installs to
`/usr/lib/omaspace-driver/`, runs as `omaspace-cua-driver.service`; `omaspace
setup` installs `~/.config/hypr/cua.lua` and the allowlist. The plugin is
built for Hyprland 0.56.2: a Hyprland upgrade needs the package rebuilt.

## Agents

omaspace exposes an MCP server (`omaspace mcp`, stdio) with tools
`list_peers`, `snapshot`, `send_workspace`, `pull_workspace`. Clicking and
typing inside windows is cua-driver's job (MIT, already on Omarchy as
`cua-driver-bin`); omaspace's MCP tells agents which machine and workspace a
window is on so they can point cua-driver at it.

## Chromium session format (reference notes)

`Sessions/Session_<n>`: `"SNSS"` + int32 version (3), then commands
`uint16 size | uint8 id | payload`. Used ids: 0 SetTabWindow(window, tab),
6 UpdateTabNavigation (pickle: uint32 len, int32 tab, int32 index, string16? url
as int32 len + UTF-8, …), 7 SetSelectedNavigationIndex(tab, index),
8 SetSelectedTabInIndex(window, index), 2 SetTabIndexInWindow(tab, index),
16 TabClosed, 17 WindowClosed. A window's tabs are the tabs whose last
SetTabWindow points at it and that were not closed; a tab's URL is its
navigation at the selected index. omaspace only reads this file (copied first,
Chromium keeps it open).

## Live view (omaspace view)

An Omarchy-native remote: watch and drive another machine's desktop from a
browser (laptop or phone), with Omarchy's workspaces, windows and bindings as
first-class controls, and the agent as a visible second participant.

### Streaming pipeline (chosen for efficiency)

| Stage | Choice | Why |
|---|---|---|
| Capture | Hyprland output capture on the GPU (`gpu-screen-recorder`, DMA-BUF, damage-driven: frames only when pixels change) | no CPU copy of the framebuffer; idle desktop sends ~nothing |
| Encode | Hardware H.264 (VAAPI on AMD/Intel, NVENC on NVIDIA), CPU x264 fallback | ~0 CPU; H.264 decodes in every browser |
| Container | MPEG-TS on stdout, split into Annex B access units by omaspace | no muxing work client-side |
| Transport | One WebSocket per viewer, binary frames: `u8 type | u64 pts_us | payload` | works through Tailscale Serve HTTPS; no extra ports |
| Decode | WebCodecs `VideoDecoder` in the browser, painted to a canvas | hardware decode, ~1 frame of latency |
| Flow control | Drop to the next keyframe when a viewer's socket backs up | a slow phone never adds latency for itself |

Per viewer the server picks the output size: the full monitor for a laptop, a
phone-sized scale for a phone, or one window's region (`-region`) for the
window-only view. For reference, cua Spaces' viewer encodes on the CPU
(OpenH264) at ~36 ms/frame; GPU capture+encode is the bar to beat.

### Omarchy controls (JSON over the same WebSocket)

- `state`: workspaces (id, window count, focused), windows (address, class,
  title, workspace, floating, fullscreen, geometry), active window, monitor
  size, participants, pending help requests. Pushed on every Hyprland event
  (`.socket2.sock`), never polled.
- Actions: `focus_workspace`, `focus_window`, `move_window{workspace}`,
  `close_window`, `toggle_floating`, `fullscreen`, `omarchy_menu`, `launcher`,
  `terminal`, `key{mods,key}`, `text`, `pointer{x,y,buttons}`, `scroll`.
  Pointer/keys go through a Wayland virtual pointer/keyboard
  (`wlr-virtual-pointer`, `virtual-keyboard`), which Hyprland supports.
- `give{peer}`, `take{peer, sign_ins}`: omaspace workspace transfer.

### Sharing

- Every viewer is a participant: `{id, name, kind: human|agent, cursor}`.
- Each participant's last pointer position is broadcast and drawn as its own
  labeled cursor; the agent's (cua-driver actions, reported over
  `POST /v1/view/agent`) in a distinct color.
- `help{message}` from an agent raises a banner on every human viewer and an
  Omarchy notification on the machine; `take_over` pauses agent input
  (`/v1/view/agent` refuses) until `hand_back`.

### Security

Same trust rule as the rest of omaspace: the view endpoint is only on the
tailnet and every connection is checked with tailscaled whois. The machine
itself is never trusted: a connection from it to its own tailnet address (any
local process can make one) is identified as this node and refused. Browsers reach
it through `tailscale serve --https=7788 unix:$XDG_RUNTIME_DIR/omaspace-view.sock`
(HTTPS is needed for WebCodecs' secure context). The view listens only on that
unix socket (0600, in the user's 0700 runtime directory), never on TCP: a
loopback port would let any other user on the machine connect and send a
forged `X-Forwarded-For`. Tailscale Serve forwards the caller's tailnet
address in `X-Forwarded-For` (a client-sent value is replaced, not passed
through); the view server runs whois
on the last entry and refuses on any failure.

A request from a browser must also carry this view's own `Origin`
(`https://<host>.<tailnet>:7788`). Without that check, any web page open on one
of your devices could open the WebSocket (browsers do not apply CORS to
WebSockets) or send a simple POST, with your device's tailnet identity, and
drive the desktop. Requests with no `Origin` (curl, agents) are not from a web
page and only need to pass the trust rule. The one cross-origin read is
`GET /v1/view/presence`, for the machine switcher, and only another
`https://<host>.<tailnet>:7788` page may read it.

## Files (put / get / sync)

Same feature set as cua Spaces' file features (CLI copy, viewer drag and
drop, two-way folder sync), reimplemented from the idea, not the code.

### Transfer endpoints (on the daemon, :7787, same trust rule)

| Method | Path | |
|---|---|---|
| GET | `/v1/files/list?path=~/Downloads` | directory listing: name, size, mtime, dir |
| GET | `/v1/files/get?path=…` | stream a file (supports `Range:` for resume) |
| PUT | `/v1/files/put?path=…&offset=N` | write the body at offset N; `offset=0` truncates |
| POST | `/v1/files/commit?path=…&size=S&sha256=H` | verify size + SHA-256, rename `.omaspace-part` into place |

Uploads land in `<path>.omaspace-part` first and only appear under their real
name after `commit` verifies them, so a half-sent file never looks finished.
A resumed upload asks `list` for the part's size and continues from there.
Chunks are 8 MiB.

Paths: `~/…` relative to the receiving user's home; must resolve inside
`$HOME` (no `..`, no symlink escape) and not inside a dot-folder at the top
level (`~/.ssh`, `~/.config`, `~/.local/…` are refused). Default destination
is `~/Downloads`. Name clashes get ` (2)`, ` (3)`, … like a browser.

### CLI

```
omaspace put <peer> <file|dir>... [--to ~/Downloads]
omaspace get <peer> <remote path>... [--to .]
omaspace ls  <peer> [path]
omaspace sync <peer> <local dir> [remote dir]   # two-way, until Ctrl-C (or --once)
```

Directories are walked and sent file by file. Each file prints one line;
any failure exits non-zero with the file named, never skipped silently.

### Two-way sync (three-way merge, cua folderSync's rule)

State file `~/.local/state/omaspace/sync/<peer>-<hash of both paths>.json`
holds the *base*: each relative path's (size, mtime, sha256) on both sides at
the last successful pass. Each pass lists both sides and decides per path:

| local vs base | remote vs base | action |
|---|---|---|
| changed | same | copy local → remote |
| same | changed | copy remote → local |
| changed | changed, equal content | nothing (both did the same) |
| changed | changed, different | **conflict**: keep both; the remote copy is saved as `name (conflict from <peer>).ext` |
| deleted | same | delete remote |
| same | deleted | delete local |
| new | absent | copy local → remote; and vice versa |

Deletions only propagate for paths that were in the base (never on first
sync). Dot-files and `node_modules`, `target`, `.git` are skipped.

### In the viewer

- Desktop browser: drop files onto the view → put into the remote's
  `~/Downloads` (progress toast per file).
- Phone: a Files button → upload (photo/file picker) and a `~/Downloads`
  list to download from.
- Files go over the daemon's file endpoints through the view server (same
  identity check), so the phone never needs a second connection.

### Omarchy menu

`omarchy-omaspace` gains "Send file to…" (Omarchy's file picker, then the
machine) and "Sync a folder with…".
