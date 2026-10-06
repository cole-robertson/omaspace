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
- **The live view has no network port of its own.** It listens on a unix socket only you can
  open (`$XDG_RUNTIME_DIR/omaspace-view.sock`, mode 0600), published on your tailnet with
  `tailscale serve`. Other users on the same machine can't reach it.
- **Other websites can't drive it.** Browser requests to the live view must come from its own
  page (`Origin` is checked), so a web page open on one of your devices can't open its
  WebSocket and type into your desktop.
- **Agent browsers' DevTools stay private.** An agent's browser holds a copy of your sign-ins.
  Its DevTools runs over a pipe relayed on a 0600 unix socket, never a TCP port that other local
  users could connect to.
- **Sign-ins are opt-in.** Cookies, saved passwords and history stay behind unless you allow
  them for a transfer.
- **Restores never run a shell string.** A snapshot can only reopen windows through fixed
  launchers (the browser with http(s) URLs, the terminal in a directory, an installed `.desktop`
  file), with every argument quoted.
- **File transfers stay in your home folder,** out of top-level hidden folders and out of
  folders that tools run code from (`.git`, `.envrc`, `.vscode`, …).
