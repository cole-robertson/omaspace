# Changelog

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
