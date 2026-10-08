# omaspace end-to-end tests

Playwright tests that drive real Omarchy desktops: they open windows, type with
virtual input devices, watch the live view from phone- and laptop-sized
browsers, run agents and move files between machines. They need a real
Hyprland session, so they don't run in hosted CI; run them on demand.

## Machines

| Variable | |
|---|---|
| `OMASPACE_E2E_DESK` | **Required.** Tailnet hostname of the Omarchy machine under test: its live view, agents, Spaces panel. |
| `OMASPACE_E2E_PEER` | Optional. A second machine for workspace and file transfers. Tests that need it are skipped without it. |
| `OMASPACE_E2E_CHROMIUM` | Optional. Chromium for the test browsers (default `/usr/bin/chromium`). It needs the H.264 decoder, which Playwright's bundled Chromium lacks. |

Run the tests from any machine on the tailnet; a machine that isn't the one
running the tests is reached with `ssh <hostname>` (Tailscale SSH works).

Each machine needs:

- Omarchy, with `omaspace setup` done (the daemon, live view and Spaces panel);
- the live view published: `omaspace setup` does it, or
  `sudo tailscale serve --bg --https=7788 unix:$XDG_RUNTIME_DIR/omaspace-view.sock`;
- the same Tailscale user as the machine running the tests (omaspace only
  trusts your own devices);
- for the desktop-app agent tests, the `omaspace-driver` package
  (`pkg/omaspace-driver`); those tests skip without it.

## Running

```sh
OMASPACE_E2E_DESK=mydesk bin/omaspace-e2e                 # every test that needs one machine
OMASPACE_E2E_DESK=mydesk OMASPACE_E2E_PEER=mylaptop bin/omaspace-e2e
bin/omaspace-e2e view                                     # one spec file
bin/omaspace-e2e -g "security"                            # Playwright flags pass through
```

The tests use workspaces 5 to 9 on the machines and close every window on
some of them first, so run them on a machine you aren't working on, or keep
your windows on workspaces 1 to 4. The agent tests restart the desk's
Chromium. Everything a test creates is cleaned up after it, pass or fail.

The HTML report, with screenshots and traces of failures, is left in
`e2e/report/`: `npx playwright show-report e2e/report`.

