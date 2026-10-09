# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through a
[GitHub Security Advisory](https://github.com/cole-robertson/omaspace/security/advisories/new)
on this repository, not in a public issue or pull request. Include the affected version or
commit, the steps to reproduce, and the impact you expect.

You'll get a reply on the advisory. Once a fix is released, the advisory is published with credit
to you unless you'd rather stay anonymous.

## Supported versions

Fixes land on `main` and in the next release.

## How omaspace is protected

omaspace can move your browser sign-ins between machines, show your screen and take keyboard
input, so it only talks to your own devices. [SPEC.md](SPEC.md) has the details; in short:

- **Only your own devices.** The daemon listens on this machine's Tailscale address only, and
  every request is identified with tailscaled's `whois` and refused unless the caller belongs to
  the same Tailscale user (or a login listed in `~/.config/omaspace/owners`). There are no tokens
  to copy around.
- **Not even this machine itself.** A connection from this machine to its own tailnet address
  is identified as this machine, and any local process could make one (another Unix user, a
  container, a sandboxed app), so both the daemon and the live view refuse it. You reach a
  machine's desktop from your other devices.
- **No DNS rebinding.** The daemon only answers requests addressed to its tailnet IP and
  refuses any request that carries an `Origin` header, so a web page can't reach it by
  pointing its own hostname at your machine or by posting a cross-site form.
- **The live view has no network port of its own.** It listens on a unix socket only you can
  open (`$XDG_RUNTIME_DIR/omaspace-view.sock`, mode 0600), published on your tailnet with
  `tailscale serve`. Other users on the same machine can't reach it.
- **Other websites can't drive it.** Browser requests to the live view must come from its own
  page (`Origin` is checked), so a web page open on one of your devices can't open its
  WebSocket and type into your desktop.
- **Agent browsers' DevTools stay private.** An agent's browser holds a copy of your sign-ins.
  Its DevTools runs over a pipe relayed on a 0600 unix socket, never a TCP port that other local
  users could connect to.
- **It can't hang your desktop.** Every call to Hyprland has a deadline, and the live view
  backs off when Hyprland doesn't answer instead of queueing more work on it. At most four
  phone-shaped virtual screens can exist, and leftovers are cleaned up one at a time in the
  background.
- **Sign-ins are opt-in, and scoped.** Cookies, saved passwords and history stay behind unless
  you allow them for a transfer, and a machine only accepts sign-ins for the sites open in the
  windows it's given.
- **Restores never run a shell string.** A snapshot can only reopen windows through fixed
  launchers (the browser with http(s) URLs, the terminal in a directory, an installed `.desktop`
  file), with every argument quoted.
- **File transfers stay in your home folder,** out of top-level hidden folders and out of
  files and folders that tools run code from (`.git`, `.envrc`, `.vscode`, `.cargo`,
  `mise.toml`, `.nvim.lua`, …). The rules apply to where a symlink really leads, a write never
  follows a symlink, and a name a peer sends back in a listing can't climb out of the folder.

## What omaspace trusts

- **Your devices fully.** Any device that passes the trust rule can see and drive the desktop,
  read and write files in your home folder, and (with your policy's permission) move sign-ins.
  On a tagged machine, that is every device sharing one of its tags, so don't share a tag with
  CI runners or servers you wouldn't hand your desktop to.
- **Agents as you.** An agent's MCP tools run as your user: `terminal_run` and `open_app` run
  any command, and an agent's browser starts with your sign-ins. The workspace an agent claims
  keeps it out of your way and is where it's expected to work, but it is not a sandbox. Give
  agents the same trust you'd give a script you run yourself. Each MCP connection acts as one
  agent, so agents can't drive each other's browsers or windows through omaspace.
