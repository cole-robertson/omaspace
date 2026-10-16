// A machine with no monitor (a box in a cupboard): Hyprland shows its
// workspaces on a FALLBACK screen of its own. Giving and taking workspaces and
// watching it from a phone and a laptop must work the same. The desk's
// monitor is switched off for these tests and back on after, and a timer on
// the desk turns it back on anyway if the run dies first.

import { test, expect, waitFor } from "../lib/fixtures.js";
import { isPhoneScreen } from "../lib/machine.js";

const WS = 7;
test.describe.configure({ mode: "serial" });

let monitor;

test.beforeAll(async ({}, info) => {
  const { Machine } = await import("../lib/machine.js");
  const desk = new Machine(process.env.OMASPACE_E2E_DESK);
  const real = desk.monitors().filter(m => !isPhoneScreen(m) && m.name !== "FALLBACK");
  info.skip(real.length !== 1, "needs a desk with exactly one monitor to switch off");
  monitor = real[0].name;
  const on = `hl.monitor({ output = "${monitor}", disabled = false, mode = "preferred", position = "auto", scale = ${real[0].scale} })`;
  desk.sh(`systemd-run --user --collect --unit=omaspace-e2e-monitor --on-active=600 --setenv=HYPRLAND_INSTANCE_SIGNATURE="$HYPRLAND_INSTANCE_SIGNATURE" hyprctl eval '${on}' >/dev/null 2>&1 || true`);
  desk.sh(`hyprctl eval 'hl.monitor({ output = "${monitor}", disabled = true })'`);
  await waitFor(() => desk.monitors().every(m => m.name !== monitor), "the monitor is off");
});

test.afterAll(async () => {
  if (!monitor) return;
  const { Machine } = await import("../lib/machine.js");
  const desk = new Machine(process.env.OMASPACE_E2E_DESK);
  desk.sh(`systemctl --user start omaspace-e2e-monitor.service 2>/dev/null; systemctl --user stop omaspace-e2e-monitor.timer 2>/dev/null; true`);
  await waitFor(() => desk.monitors().some(m => m.name === monitor), "the monitor is back on");
});

test.beforeEach(async ({ desk, peer }) => {
  await desk.clearWorkspace(WS);
  await peer.clearWorkspace(WS);
});

test("give to and take back from a machine with no monitor", async ({ desk, peer }) => {
  expect(desk.monitors().map(m => m.name)).toEqual(["FALLBACK"]);
  const dir = peer.scratch("headless");
  desk.sh(`mkdir -p ${dir.replace(/^\/home\/[^/]+/, "$HOME")}`);
  desk.onCleanup(() => desk.rm(dir.replace(/^\/home\/[^/]+/, "$HOME")));
  await peer.terminal(dir, WS);

  expect(peer.omaspace(["send", desk.name, "--workspace", String(WS)]).out).toMatch(/restored 1, skipped 0, failed 0/);
  const win = await waitFor(() => desk.windowsOn(WS).find(c => /foot|terminal/i.test(c.class)), "terminal on the headless desk");
  expect(desk.terminalCwd(win)).toBe(dir);

  await peer.clearWorkspace(WS);
  expect(peer.omaspace(["pull", desk.name, "--workspace", String(WS)]).out).toMatch(/restored 1, skipped 0, failed 0/);
  const back = await waitFor(() => peer.windowsOn(WS).find(c => /foot|terminal/i.test(c.class)), "terminal back on peer");
  peer.onCleanup(() => peer.dispatch(`hl.dsp.window.close({ window = "address:${back.address}" })`));
  expect(peer.terminalCwd(back)).toBe(dir);
  desk.dispatch(`hl.dsp.window.close({ window = "address:${win.address}" })`);
});

test("a laptop watches and types into a machine with no monitor", async ({ desk, laptop }) => {
  const dir = desk.scratch("headless-view");
  const term = await desk.terminal(dir, WS);
  const view = await laptop.open(desk);
  const s = await view.stats();
  const mon = desk.monitors().find(m => m.name === "FALLBACK");
  expect([s.width, s.height]).toEqual([mon.width, mon.height]);
  await view.tapWorkspace(WS);
  await waitFor(() => desk.activeWorkspace() === WS, "desk on the test workspace");
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  await view.type('echo "headless: ok" > typed.txt\n');
  await waitFor(() => desk.exists(`${dir}/typed.txt`), "typed.txt");
  expect(desk.read(`${dir}/typed.txt`)).toBe("headless: ok");
});

test("a phone switches workspaces on a machine with no monitor, and a laptop can join", async ({ desk, phone, laptop }) => {
  const view = await phone.open(desk, { workspace: String(WS) });
  const phoneOut = await waitFor(() => desk.monitors().find(m => isPhoneScreen(m)), "phone screen");
  await waitFor(() => desk.monitors().find(m => m.name === phoneOut.name)?.activeWorkspace.id === WS, "phone on its workspace");
  await view.tapWorkspace(8);
  await waitFor(() => desk.monitors().find(m => m.name === phoneOut.name)?.activeWorkspace.id === 8, "phone screen shows ws8");
  await view.tapWorkspace(WS);
  await waitFor(() => desk.monitors().find(m => m.name === phoneOut.name)?.activeWorkspace.id === WS, "phone screen back on ws");
  // The phone's screen may be the only one now: a laptop still gets a picture.
  const l = await laptop.open(desk);
  expect(await l.stats()).toBeTruthy();

  await view.close();
  await waitFor(() => !desk.monitors().some(m => isPhoneScreen(m)), "phone screen removed on disconnect");
  await waitFor(() => desk.monitors().some(m => m.name === "FALLBACK"), "FALLBACK screen back");
});
