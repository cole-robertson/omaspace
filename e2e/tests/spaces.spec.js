// The Spaces panel (omaspace.spaces): the top-edge machine picker, opened by
// SUPER+CTRL+SHIFT+O or by SUPER-dragging a window to the top of the screen.
// Everything here is real input on desk's desktop through virtual devices.

import { test, expect, waitFor } from "../lib/fixtures.js";

const WS = 7;
const KEY = { SUPER: 125, CTRL: 29, SHIFT: 42, O: 24, LEFT: 105, RIGHT: 106, ESC: 1, D4: 5, ENTER: 28 };

test.beforeEach(async ({ peer, desk }) => {
  desk.spaces("dismiss");
  await desk.clearWorkspace(WS);
  await peer.clearWorkspace(4);
  desk.dispatch(`hl.dsp.focus({ workspace = "${WS}" })`);
});
test.afterEach(({ desk }) => desk.spaces("dismiss"));

test("SUPER+CTRL+SHIFT+O opens Spaces with every machine, arrows and digits pick, Esc closes", async ({ peer, desk }) => {
  desk.keys(KEY.SUPER, KEY.CTRL, KEY.SHIFT, KEY.O);
  const s = await waitFor(() => { const s = desk.spacesState(); return s.opened && !s.loading && s.machines.length > 1 && s; }, "Spaces open with machines");
  expect(s.mode).toBe("keyboard");
  expect(s.machines).toEqual(expect.arrayContaining([desk.name, peer.name]));
  expect(s.machines[s.selected.machine], "starts on another machine").not.toBe(desk.name);
  expect(s.selected.ws, "starts on the current workspace").toBe(WS);

  desk.keys(KEY.D4);
  await waitFor(() => desk.spacesState().selected.ws === 4, "4 picks workspace 4");
  const before = desk.spacesState().selected.machine;
  desk.keys(KEY.RIGHT);
  await waitFor(() => desk.spacesState().selected.machine === (before + 1) % s.machines.length, "→ picks the next machine");

  desk.keys(KEY.ESC);
  await waitFor(() => !desk.spacesState().opened, "Esc closes Spaces");
});

test("Enter on this machine's workspace switches to it", async ({ desk }) => {
  desk.keys(KEY.SUPER, KEY.CTRL, KEY.SHIFT, KEY.O);
  await waitFor(() => { const s = desk.spacesState(); return s.opened && !s.loading && s.machines.length > 1; }, "Spaces open");
  const here = desk.spacesState().machines.indexOf(desk.name);
  while (desk.spacesState().selected.machine !== here) desk.keys(KEY.LEFT);
  desk.keys(KEY.D4);
  desk.keys(KEY.ENTER);
  await waitFor(() => desk.activeWorkspace() === 4, "desk switched to workspace 4");
  expect(desk.spacesState().opened).toBe(false);
});

/** SUPER-drag the window under (fx, fy) up to the top edge; resolves once Spaces is open in drag mode. */
async function dragToTop(m, fx, fy) {
  m.input(`move ${fx} ${fy}`, "sleep 100", `key ${KEY.SUPER} down`, "sleep 50", "button 272 down", "sleep 150",
    `move ${fx} ${fy / 2}`, "sleep 150", `move ${fx} 0.005`, "sleep 300");
  return waitFor(() => { const s = m.spacesState(); return s.opened && s.mode === "drag" && !s.loading && s.machines.length > 1 && s; }, "Spaces opened by dragging to the top");
}
function release(m) { m.input("button 272 up", "sleep 50", `key ${KEY.SUPER} up`); }

test("drag a window to the top edge and drop it on another machine's workspace", async ({ peer, desk }) => {
  const dir = desk.scratch("drag");
  peer.onCleanup(() => peer.rm(dir));
  const win = await desk.terminal(dir, WS);
  const [w, h] = desk.screen();

  const s = await dragToTop(desk, (win.at[0] + win.size[0] / 2) / w, (win.at[1] + win.size[1] / 2) / h);
  expect(s.dragAddress).toBe(win.address);
  const target = s.machines.indexOf(peer.name);
  const [x, y] = desk.spaces("cellCenter", `${target} 4`).split(" ").map(Number);
  desk.input(`move ${x / w} ${(y + 2) / h}`, "sleep 300");
  await waitFor(() => { const hv = desk.spacesState().hover; return hv.machine === target && hv.ws === 4; }, "hovering peer's workspace 4");
  release(desk);

  await waitFor(() => !desk.spacesState().opened, "Spaces closes on drop");
  expect(desk.spacesState().lastAction).toContain(`give-window ${peer.name} ${win.address} 4`);
  const arrived = await waitFor(() => peer.windowsOn(4).find(c => /foot|terminal/i.test(c.class)), "terminal on peer's workspace 4", 30_000);
  peer.onCleanup(() => peer.dispatch(`hl.dsp.window.close({ window = "address:${arrived.address}" })`));
  expect(peer.terminalCwd(arrived)).toBe(dir);
});

test("dropping a dragged window anywhere but a machine gives nothing", async ({ peer, desk }) => {
  const win = await desk.terminal("$HOME", WS);
  const [w, h] = desk.screen();
  await dragToTop(desk, (win.at[0] + win.size[0] / 2) / w, (win.at[1] + win.size[1] / 2) / h);
  desk.input("move 0.05 0.6", "sleep 300");
  release(desk);
  await waitFor(() => !desk.spacesState().opened, "Spaces closes");
  expect(desk.spacesState().lastAction).toBe("cancelled");
  await new Promise(r => setTimeout(r, 3000));
  expect(peer.windowsOn(4)).toHaveLength(0);
});

test("a normal SUPER-drag that never reaches the top doesn't open Spaces", async ({ desk }) => {
  const win = await desk.terminal("$HOME", WS);
  const [w, h] = desk.screen();
  const fx = (win.at[0] + win.size[0] / 2) / w, fy = (win.at[1] + win.size[1] / 2) / h;
  desk.input(`move ${fx} ${fy}`, `key ${KEY.SUPER} down`, "button 272 down", "sleep 150", `move ${fx + 0.05} ${fy + 0.05}`, "sleep 400");
  expect(desk.spacesState().opened).toBe(false);
  release(desk);
});
