// A hub: omaspace on a machine with no desktop session (a plain Linux
// server). It keeps snapshots for later; giving a workspace to it is refused.
// The desk plays the hub: its daemon is stopped and a second one started
// without the desktop's environment, then the real one is started again.

import { test, expect, waitFor } from "../lib/fixtures.js";

const WS = 7;
test.describe.configure({ mode: "serial" });

test.beforeAll(async () => {
  const { Machine } = await import("../lib/machine.js");
  const desk = new Machine(process.env.OMASPACE_E2E_DESK);
  desk.sh(`systemctl --user stop omaspace
    env -i HOME="$HOME" PATH="$PATH" setsid omaspace serve >/dev/null 2>&1 < /dev/null &`);
});

test.afterAll(async () => {
  const { Machine } = await import("../lib/machine.js");
  const desk = new Machine(process.env.OMASPACE_E2E_DESK);
  desk.sh(`pkill -u "$USER" -f '^omaspace serve$' || true; systemctl --user start omaspace`);
});

test.beforeEach(async ({ peer }) => {
  await peer.clearWorkspace(WS);
});

test("a hub shows as one, keeps a stashed workspace, and gives it back", async ({ peer, desk }) => {
  await waitFor(() => new RegExp(`^${desk.name}\\s+hub`, "m").test(peer.omaspace(["peers"]).out), "the desk answers as a hub");

  const dir = peer.scratch("hub");
  await peer.terminal(dir, WS);
  const stored = JSON.parse(peer.omaspace(["stash", desk.name, "--workspace", String(WS)]).stdout);
  expect(stored.windows).toBe(1);
  desk.onCleanup(() => desk.rm(`$HOME/.local/state/omaspace/stash/${stored.stored}.json`));
  const list = JSON.parse(peer.omaspace(["stashes", desk.name]).stdout);
  expect(list.map(s => s.id)).toContain(stored.stored);

  await peer.clearWorkspace(WS);
  expect(peer.omaspace(["restore", `${desk.name}:${stored.stored}`]).out).toMatch(/restored 1, skipped 0, failed 0/);
  const back = await waitFor(() => peer.windowsOn(WS).find(c => /foot|terminal/i.test(c.class)), "terminal back from the hub");
  peer.onCleanup(() => peer.dispatch(`hl.dsp.window.close({ window = "address:${back.address}" })`));
  expect(peer.terminalCwd(back)).toBe(dir);
});

test("giving to or taking from a hub says it has no desktop", async ({ peer, desk }) => {
  await peer.terminal("$HOME", WS);
  const give = peer.omaspace(["send", desk.name, "--workspace", String(WS)], { check: false });
  expect(give.code).not.toBe(0);
  expect(give.out).toMatch(/no desktop session/);
  const take = peer.omaspace(["pull", desk.name, "--workspace", String(WS)], { check: false });
  expect(take.code).not.toBe(0);
  expect(take.out).toMatch(/no desktop session/);
});
