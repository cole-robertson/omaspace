// Giving and taking back workspaces between two Omarchy machines.

import { test, expect, waitFor } from "../lib/fixtures.js";

const WS = 7; // a workspace the tests own on both machines

test.beforeEach(async ({ peer, desk }) => {
  await peer.clearWorkspace(WS);
  await desk.clearWorkspace(WS);
});

test("peers: each machine sees the other as a desktop", async ({ peer, desk }) => {
  expect(peer.omaspace(["peers"]).out).toMatch(new RegExp(`^${desk.name}\\s+desktop`, "m"));
  expect(desk.omaspace(["peers"]).out).toMatch(new RegExp(`^${peer.name}\\s+desktop`, "m"));
});

test("give: a terminal reopens on the same workspace and folder on the other machine", async ({ peer, desk }) => {
  const dir = peer.scratch("give");
  desk.sh(`mkdir -p ${dir.replace(/^\/home\/[^/]+/, "$HOME")}`);
  desk.onCleanup(() => desk.rm(dir.replace(/^\/home\/[^/]+/, "$HOME")));
  await peer.terminal(dir, WS);

  const before = desk.activeWorkspace();
  const { out } = peer.omaspace(["send", desk.name, "--workspace", String(WS)]);
  expect(out).toMatch(/restored 1, skipped 0, failed 0/);

  const win = await waitFor(() => desk.windowsOn(WS).find(c => /foot|terminal/i.test(c.class)), "terminal on desk");
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.close({ window = "address:${win.address}" })`));
  expect(desk.terminalCwd(win)).toBe(dir);
  expect(desk.activeWorkspace(), "giving must not steal the other machine's focus").toBe(before);
});

test("take back: browser tabs and a terminal come back from the other machine", async ({ peer, desk }) => {
  await desk.browser(["https://example.org/", "https://www.rfc-editor.org/"], WS);
  await desk.terminal("$HOME", WS);

  const { out } = peer.omaspace(["pull", desk.name, "--workspace", String(WS)]);
  expect(out).toMatch(/restored 2, skipped 0, failed 0/);

  const wins = await waitFor(() => { const w = peer.windowsOn(WS); return w.length >= 2 && w; }, "2 windows on peer");
  for (const w of wins) peer.onCleanup(() => peer.dispatch(`hl.dsp.window.close({ window = "address:${w.address}" })`));
  expect(wins.map(w => w.class).sort()).toEqual(expect.arrayContaining(["chromium"]));
  // Chromium writes its session file a few seconds after tabs open.
  const tabs = () => JSON.parse(peer.omaspace(["snapshot", "--workspace", String(WS)]).stdout)
    .workspaces[0].windows.find(w => w.kind === "browser")?.urls;
  await expect.poll(tabs, { timeout: 30_000 }).toEqual(["https://example.org/", "https://www.rfc-editor.org/"]);
});

test("give --with-files carries the project folder's contents", async ({ peer, desk }) => {
  const dir = peer.scratch("proj");
  peer.write(`${dir}/README.md`, "from peer");
  await peer.terminal(dir, WS);
  desk.onCleanup(() => desk.rm(dir));

  peer.omaspace(["send", desk.name, "--workspace", String(WS), "--with-files"]);
  expect(desk.read(`${dir}/README.md`)).toBe("from peer");
  const win = await waitFor(() => desk.windowsOn(WS)[0], "terminal on desk");
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.close({ window = "address:${win.address}" })`));
});

test("snapshots never contain cookies unless sign-ins are allowed", async ({ peer }) => {
  await peer.browser(["https://example.org/"], WS);
  const snap = JSON.parse(peer.omaspace(["snapshot", "--workspace", String(WS)]).stdout);
  for (const data of Object.values(snap.browser_data || {})) {
    expect(data.cookies ?? []).toHaveLength(0);
    expect(data.allowed_sensitive ?? []).toHaveLength(0);
  }
});
