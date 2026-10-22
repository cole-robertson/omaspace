# Changelog

## 0.3.5 - 2026-10-22

- **An agent can work in more than one space.** Each agent's connection was
  locked to the first name it used, so an agent already holding a workspace
  couldn't open a second one for a second task. It now claims another space
  under a new name (`claim_space` with, say, "Claude 2") and works there
  under that name. It still can't act as a name it didn't claim, and
  claiming another agent's name is refused.

## 0.3.4 - 2026-10-21

- **Setup on a machine that isn't signed in to Tailscale says so.** It
  carried on, printed a JSON error ("invalid type: null, expected a
  sequence") and left the services restarting with the same error. Setup
  now stops before installing anything, and every command says: *Tailscale
  isn't connected; sign in with: sudo tailscale up*.

## 0.3.3 - 2026-10-19

- **An agent's browser opens without "Restore pages?".** It starts from a copy
  of your sign-ins, taken while your Chromium is running, so the copy said
  Chromium hadn't closed cleanly and the agent's first page opened under
  Chromium's restore prompt.
- **The README shows each feature in motion:** giving a workspace, the phone
  view, an agent asking for help, swapping windows; more in
  [docs/demos.md](docs/demos.md).

## 0.3.2 - 2026-10-16

- **The live view works on a machine with no monitor.** It mirrored the
  screen it found at startup, so unplugging the monitor (or a machine that
  never had one, where Hyprland shows its own FALLBACK screen) left gliff
  looking for a screen that was gone. It now asks which screen is real each
  time. A phone switching workspaces there no longer tries to hand the old
  one to a screen that isn't there, and a laptop joining while a phone's
  screen is the only one watches that.
- **Tests for a machine with no monitor and for a hub:** giving to and taking
  from a headless machine, watching it from a phone and a laptop, and
  stashing to and restoring from a hub.
- **Steadier end-to-end tests on a busy machine.** The tile tests wait for
  the phone's screen to get its bar and for windows to stop moving before
  dragging, and hold until a tile lifts instead of a fixed time; the arrow
  pad and agent hand-over tests wait for what they check.

## 0.3.1 - 2026-10-15

Fixes found testing with gliff 0.4.0.

- **Setup checks the gliff the live view will run:** the first `gliff-server`
  on the PATH omaspace's services use, not the shell's. It says which one
  (version and path), and refuses one only the shell can find.
- **gliff's check leaves no screen behind.** It stopped gliff before gliff
  removed its private screen, so every setup left one holding a workspace
  until the sweep caught it, and a phone that connected then could land on
  the wrong workspace.
- **The phone page fits the phone.** A full top bar (presence, agents) made
  the page wider than the phone, so it asked for the wrong size; the
  workspaces scroll instead.
- **Phone screen sizes match gliff's rounding** (gliff 0.4 rounds to an even
  step), so a resize lands on the size the page expects.

## 0.3.0 - 2026-10-14

**The live view runs only on gliff.** omaspace's own screen recorder is gone
(about 900 lines): gliff is faster, sharper and steadier on every measure, so
there's no reason to keep a second way.

- **Setup installs gliff,** or uses the one already installed. It installs
  Omarchy's package, signed with the key the machine already trusts for its
  updates, from the stable repository or, until gliff reaches stable, from
  edge. Setup and the live view check that gliff speaks omaspace's protocol
  version, and the page says so when it doesn't, instead of reconnecting.
- **Agents set up themselves.** Setup registers the MCP server with Claude
  Code, Codex and opencode (`omaspace mcp add` again later), and `omaspace
  mcp` finds the desktop on its own, so agents started by a service need no
  wrapper script. A registration that's already there is kept by setup.
- **Phone screens left behind are cleaned up** by gliff's names too (only
  ones whose gliff-server has exited).
- **Scales like 1.4 and 1.8** find their clean size (the same fix as
  omacom/gliff#46).
- **Removed:** "View only this window" (gliff streams whole screens; the
  window's workspace is the way to watch it) and the wf-recorder dependency.

## 0.2.0 - 2026-10-12

**The live view runs on [gliff](https://github.com/omacom/gliff),** Omarchy's
remote desktop. omaspace starts `gliff-server` for each viewer and brings it
to the browser; it falls back to its own recorder when gliff isn't installed.

- **Much faster.** gliff captures and encodes on the GPU and sends a frame as
  soon as the screen changes. Measured key press to picture on the same
  network: 13-18 ms, where the old recorder took 70 ms on a phone and up to
  600 ms on a laptop (sometimes never updating).
- **Sharp text in a laptop's browser.** Full 4:4:4 colour: both of gliff's
  streams decode in the browser and a WebGL2 shader recombines them. Phones
  use a single stream to save bandwidth.
- **Copy and paste between your device and the machine** (text, both ways).
- **Phones get gliff's private screen,** sized to the phone; a laptop opened
  at the same time still mirrors the real monitor. Rotating the phone or
  adding the view to the home screen resizes that screen in place, so the
  workspace on it never moves, and the workspace is pinned to the phone's
  screen while it's connected (Hyprland would otherwise destroy an empty one
  mid-move and reopen it elsewhere).
- **An agent's browser never waits on a locked keyring.** It runs without the
  keyring (its copy of your sign-ins uses Chromium's built-in key), and
  reading your sign-ins gives the keyring 5 seconds: before, a locked keyring
  hung `open_browser` and every page load until someone unlocked it.
- **Agent screenshots can't crash Hyprland.** Capturing a window on a
  workspace no monitor shows crashed Hyprland 0.56.2; `app_read` now skips the
  screenshot there and returns the accessibility tree.

## 0.1.6 - 2026-10-08

- **Tagged machines work from your phone after setup.** A machine owned by
  a Tailscale tag trusts no person's devices until told; `omaspace setup` now
  asks whose devices may use it and writes `~/.config/omaspace/owners`, and
  a refused device is told why and how to fix it.
- **`omaspace setup` installs what the phone view needs** (`wf-recorder`),
  so a fresh machine's phone view is phone-shaped without a manual step.
- **A screen that's off can't fool you.** When a machine idles, its display
  powers off and the stream stops on the last picture, so a viewer could
  type into a stale image without seeing anything happen. Now the view says
  *The screen is off: this is the last picture* with a **Wake it** button;
  any tap or key from a viewer also turns the screen back on (Hyprland
  doesn't count input from virtual devices as activity), and opening the
  view does too.
- **The recorder's errors are in the view's log** instead of discarded, so a
  stream that won't start says why.
- **MCP tools that return a list work with strict clients**
  (`structuredContent` is always an object; a list goes under `items`).
- `omaspace sync add` records the first pass, so `sync list` shows it at once.
- The end-to-end suite refuses a locked machine and keeps screens awake.

## 0.1.5 - 2026-10-08

- **Folder sync is a real feature now.** `omaspace sync add <machine> <folder>`
  keeps a folder the same on both machines in the background: the omaspace
  service runs it, so it carries on after a restart, and retries with backoff
  while the other machine is off. `omaspace sync list` shows each folder and
  how it's doing, `omaspace sync remove` stops one (the files stay).
- **Files everywhere you are.** The Omarchy menu adds *Get files from…* and
  *Synced folders*; the Spaces panel shows how many folders each machine keeps
  in sync with you (and if one is failing), with `G` to get files and `Y` for
  synced folders; the live view's Files panel marks synced folders and shows
  their status; agents get `list_files`, `send_file`, `get_file` and
  `synced_folders`. `omaspace ls --names` prints plain file names for scripts.
- **Easier install.** The README's commands always fetch the latest release,
  and `omaspace setup` asks once to let Tailscale publish the live view
  instead of telling you to run it yourself.
- **Security:** at most 8 video encoders run at once, and stream widths snap
  to a few sizes so viewers share them; request heads are capped in size and
  time, and connections in number. Another machine's switcher only sees that
  an agent needs you, not what it asked. A machine only accepts sign-ins for
  sites open in the windows it's given. Each MCP connection is one agent, so
  an agent can't act as another.

## 0.1.4 - 2026-10-08

Security hardening before launch.

- **This machine is no longer trusted as one of your devices.** A connection
  from a machine to its own tailnet address is identified as that machine, so
  any local process (another Unix user, a container) could reach the daemon
  and the live view and pass the trust rule. Both now refuse it.
- **The daemon refuses browser requests.** It only answers requests addressed
  to its tailnet IP that carry no `Origin`, so a web page can't reach it by DNS
  rebinding or with a cross-site form.
- **File transfers can't be redirected by symlinks.** The hidden-folder and
  blocklist rules apply to where a path really leads; part files are opened
  without following symlinks; and names a peer sends in a listing (sync,
  `get`) must be plain names, so they can't point outside the folder.
- **More auto-run files are refused** for transfers: `.cargo`, `.npmrc`,
  `mise.toml`, `.tool-versions`, `.nvim.lua`, `.exrc`, `.zed`, `.devcontainer`
  and others.
- Lua strings sent to Hyprland use an explicit escaper; moving a window only
  accepts a numbered workspace; `notify-send` text can't be read as options.
- SECURITY.md now says plainly what omaspace trusts, including that agents
  run as you.

## 0.1.3 - 2026-10-08

- **Phone screens no longer pile up in the display list.** Each phone gets a
  virtual `OSP-PHONE-…` screen. One that had been switched off (by a monitor
  config reload or a display panel toggle) couldn't be removed, so they
  collected in Omarchy's display panel until Hyprland restarted. The view now
  switches such a screen back on and removes it, and every 30 seconds it
  sweeps away any phone screen no viewer is using, including ones left from
  before it started.
- **Phones stream at a usable width.** A phone screen is now at least 520
  logical pixels wide (the scale is lowered to fit), so browsers no longer
  hang off the right edge and terminals aren't cramped.
- **The machine switcher works on phones.** Taps on it were swallowed by the
  stage's touch handling.
- **Phone mode needs wf-recorder;** without it the view falls back to the
  whole desktop instead of failing, and a phone screen is removed if its
  capture can't start.
- **Taking over answers an agent's request for help,** so it isn't shown
  again after you hand back.

## 0.1.2 - 2026-10-06

- **Swapping tiles on a phone lands cleanly.** When you let go, the window
  you dragged glides into its new slot and stays there until the live video
  shows the swap finished, then fades. Before, it faded on a fixed timer
  while Hyprland was still sliding the windows, so for a moment you saw them
  half-moved or back in their old places.

## 0.1.1 - 2026-10-06

- **Take over works on phones.** Hand back was in the phone's folded-away
  actions, so after taking over there was no way to resume the agent. While
  someone has control, a bar at the top of the screen says so and has
  **Hand back**, on every screen size.
- **Take over pauses agents for real.** It now pauses every agent's MCP tools
  on that machine, not only agents reporting a cursor: actions are refused
  with a message to wait, while reading, status and asking for help still
  work. `get_space` shows who has taken over. Closing the view, or restarting
  it, hands back.
- **An agent's call for help reaches you on any workspace,** not only when
  you're watching the agent's own workspace.
- **The live view can't stall the desktop or itself on Hyprland.** Every
  `hyprctl` call has a deadline, and after one times out the view stops
  sending more until Hyprland answers again. Leftover phone screens are
  removed one at a time in the background at startup, at most four phone
  screens can exist at once, and their names are unique per run. Connecting
  input to a stuck compositor gives up instead of blocking every viewer. A
  slow phone link is backpressure, not a disconnect, and its buffer is capped.
- **Taps during a reconnect aren't lost:** actions wait and go out when the
  connection is back. Window actions from a tile's sheet act on that window
  even if the layout changed meanwhile, and quick swaps in a row swap the
  windows you dragged.
- Dead recorder processes are reaped; an agent's desktop-app session that the
  driver ended is started again.

## 0.1.0 - 2026-10-06

First public release.

- **Give and take back workspaces** between your Omarchy machines over Tailscale: browser
  windows (tabs, site data, and sign-ins when you allow them), terminals and tmux sessions,
  editors and apps reopen on the same workspace. `--with-files` copies the project folders.
  A headless Linux hub can keep snapshots for later.
- **Live view** of any of your machines in a browser: H.264 video, keyboard, mouse and touch
  input, a phone-shaped virtual screen, workspace switching, swipe gestures, file upload and
  download, hold-to-talk dictation, and a switcher for all your machines. Installable as a
  home-screen app.
- **Agent spaces** over MCP: an agent gets its own workspace with a browser signed in as you,
  a terminal and desktop apps (with `omaspace-driver`), and never takes your focus. Watch it,
  answer its help requests, hand it a browser or take one back.
- **Files:** `put`, `get`, `ls`, two-way `sync`, and drag and drop in the live view.
- **Omarchy integration:** keybindings, menu, notifications, and the Spaces panel (drag a
  window to the top edge to give it to another machine).
