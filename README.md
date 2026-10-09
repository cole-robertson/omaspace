# omaspace

Move [Omarchy](https://omarchy.org) workspaces between your own machines over
[Tailscale](https://tailscale.com), watch and drive a desktop from your phone,
and give AI agents a workspace of their own next to yours.

- **Give and take back workspaces.** Send the workspace you're on to your
  laptop: its browser windows (tabs, and optionally sign-ins), terminals (same
  directory, same tmux session) and editors reopen there, on the same
  workspace. Pull it back later. `--with-files` brings the project folders
  along.
- **Live view.** Watch any of your machines from a phone or another computer's
  browser, with real keyboard, mouse and touch input, a phone-shaped virtual
  screen, workspace switching, file upload and download, and hold-to-talk
  dictation.
- **Agent spaces.** An MCP server gives agents their own Omarchy workspace,
  with a browser signed in as you, a terminal and desktop apps, that never
  takes your focus or pointer. You can watch them from your phone, and hand
  a browser over or take it back. When an agent asks for help (a 2FA code, a
  decision), your phone shows it: **Take over** pauses every agent on that
  machine while you do it, and **Hand back** lets them carry on. Closing the
  view hands back too, so an agent is never left paused by a phone you put
  away.
- **Files and folder sync.** Copy files and folders between machines from the
  command line, the Omarchy menu, the Spaces panel or your phone, and keep
  folders the same on two machines, both ways, in the background. Everything
  goes straight between your machines, never through a cloud.

![The live view in a laptop's browser](docs/screenshots/laptop.webp)

<p align="center">
  <img src="docs/screenshots/phone-help.webp" alt="On a phone: an agent asks which time works, with Take over and Later" width="300">
  &nbsp;&nbsp;
  <img src="docs/screenshots/phone-control.webp" alt="After Take over: you're in control and the agent is paused, with Hand back" width="300">
</p>

It works only between your own devices: every request is identified with
Tailscale and refused unless it comes from your own account. Nothing goes
through a third-party server. See [SECURITY.md](SECURITY.md).

Linux only; built for Omarchy on Hyprland. A plain Linux server can join as
a *hub* that keeps snapshots for later.

## Install

You need Omarchy and Tailscale, signed in to the same Tailscale account on
each machine. Then, on each machine:

```sh
curl -fsSL https://github.com/cole-robertson/omaspace/releases/latest/download/omaspace-x86_64-linux.tar.gz | tar xz
install -m755 omaspace-*-x86_64-linux/omaspace ~/.local/bin/
omaspace setup
```

`omaspace setup` asks for your password once, to let Tailscale publish the
live view. When it's done, it prints your live view's address, and
`omaspace peers` lists your other machines.

If a machine is *tagged* in Tailscale (owned by a tag rather than by you),
setup asks whose devices may use it, so your phone and laptops are trusted
there; that list lives in `~/.config/omaspace/owners`.

Or build it from source with a Rust toolchain:

```sh
git clone https://github.com/cole-robertson/omaspace
cd omaspace
cargo build --release
install -m755 target/release/omaspace ~/.local/bin/
omaspace setup
```

`omaspace setup` installs and starts the user services, publishes the live
view on your tailnet, and adds the Omarchy keybindings, menu and Spaces panel.
Run it again any time; it only replaces what it installed. To update, repeat
the three commands above.

Agents using desktop apps (beyond their browser and terminal) also need the
`omaspace-driver` package, which builds cua-driver with Omarchy patches:
`cd pkg/omaspace-driver && makepkg -si`.

## Use

```sh
omaspace peers                        # your machines running omaspace
omaspace send laptop                  # give this workspace to "laptop"
omaspace send laptop --with-files     # ...with its project folders
omaspace pull desk --workspace 3      # take workspace 3 back from "desk"
```

## Files and folder sync

```sh
omaspace put laptop report.pdf        # into laptop's ~/Downloads
omaspace put laptop ~/photos --to ~/Pictures
omaspace get laptop ~/Downloads/scan.pdf
omaspace ls laptop ~/Documents

omaspace sync add laptop ~/notes      # keep ~/notes the same on both, from now on
omaspace sync list                    # what's in sync, and how it's going
omaspace sync remove ~/notes          # stop (the files stay on both machines)
```

- **Copies are safe to interrupt.** A file arrives under a temporary name and
  only gets its real name once its size and checksum match; an interrupted
  copy resumes. Nothing is overwritten: a clash becomes `report (2).pdf`.
- **Synced folders keep going.** The omaspace service syncs them every few
  seconds, also after a restart, and retries when the other machine is off.
  An edit, a new file or a deletion on either side goes to the other. If both
  sides changed the same file, both are kept: theirs is saved beside yours as
  `notes (conflict from laptop).md`. Hidden files, `.git`, `node_modules` and
  `target` aren't synced.
- **From Omarchy:** the Omarchy menu has *Send files to…*, *Get files from…*,
  *Sync a folder with…* and *Synced folders*; in the Spaces panel, pick a
  machine and press `F` (send), `G` (get), `S` (sync a folder) or `Y` (synced
  folders). Each machine shows how many folders it keeps in sync with you.
- **From your phone:** in the live view, **Files** browses that machine's home
  folder, uploads and downloads, and marks the folders that are in sync.
- **Agents** get `list_files`, `send_file`, `get_file` and `synced_folders`.
- Files stay inside your home folder, out of hidden folders like `~/.ssh` and
  out of files that tools run code from (`.git`, `.envrc`, …).

From Omarchy: `SUPER+CTRL+SHIFT+O` opens the Spaces panel (give, take back,
watch your machines), and so does dragging a window to the top edge with
`SUPER` held. `SUPER+CTRL+ALT+O` gives the current workspace to another
machine.

The live view is at `https://<machine>.<your-tailnet>.ts.net:7788` from any of
your devices; add it to your phone's home screen.

For agents, add the MCP server to your agent's config:

```json
{ "mcpServers": { "omaspace": { "command": "omaspace", "args": ["mcp"] } } }
```

Run `omaspace` with no arguments for every command.

## Sign-ins

A browser window's tabs, bookmarks, preferences and site data travel with it.
Cookies, saved passwords and history are withheld unless you allow them, per
transfer (`--allow-sensitive cookies`, or the Omarchy menu asks) or ahead of
time in `~/.config/omaspace/policy.json`. Cookies are decrypted on the sender
and re-encrypted with the receiver's own key; they're never copied as raw
files.

## Development

```sh
cargo test                            # unit and integration tests
cargo clippy --all-targets -- -D warnings
```

[SPEC.md](SPEC.md) describes the design, the snapshot format and the APIs.

The end-to-end suite in [e2e/](e2e/README.md) drives real Omarchy desktops
(windows, input, the live view, agents and transfers), so it runs on demand
against machines you name rather than in CI:

```sh
OMASPACE_E2E_DESK=mydesk OMASPACE_E2E_PEER=mylaptop bin/omaspace-e2e
```

## License

MIT. `pkg/omaspace-driver` builds [cua-driver](https://github.com/trycua/cua)
(MIT) with two patches.
