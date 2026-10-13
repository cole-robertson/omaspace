// Phone tiles: long-press a window to lift it, drag onto another to swap
// them, or close it from the lifted window's actions. Real touch events on a
// phone-shaped screen; Hyprland's own layout is checked afterwards.

import { test, expect, waitFor } from "../lib/fixtures.js";
import { isPhoneScreen } from "../lib/machine.js";

const WS = 8;

test.beforeEach(async ({ desk }) => {
  await desk.clearWorkspace(WS);
  desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
});

/** Centre of a window on the page, from the viewer's own outline math. */
async function centre(page, address) {
  return page.evaluate(a => {
    const w = state.windows.find(x => x.address === a), t = tileRect(w), r = document.getElementById("stage").getBoundingClientRect();
    return { x: r.left + t.x + t.w / 2, y: r.top + t.y + t.h / 2 };
  }, address);
}

async function touch(page) {
  const cdp = await page.context().newCDPSession(page);
  const at = p => [{ x: p.x, y: p.y }];
  return {
    down: p => cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: at(p) }),
    move: p => cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: at(p) }),
    up: () => cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] }),
  };
}

async function twoTiles(desk, phone) {
  desk.dispatch(`hl.dsp.focus({ workspace = "${WS}" })`);
  const view = await phone.open(desk, { workspace: String(WS) });
  await waitFor(() => desk.monitors().find(m => isPhoneScreen(m))?.activeWorkspace.id === WS, "phone screen on the test workspace");
  // Opened after the phone screen exists, so Omarchy tiles them for it.
  const a = await desk.terminal("$HOME", WS);
  const b = await desk.terminal("$HOME", WS);
  await view.page.waitForFunction(n => state.active_workspace === n && state.windows.filter(w => w.workspace === n).length >= 2 && outputOrigin.w > 0, WS, { timeout: 15_000 });
  await view.page.waitForTimeout(400);
  return { a, b, view };
}

test("long-press a tile and drag it onto another to swap them", async ({ desk, phone }, info) => {
  const { a, b, view } = await twoTiles(desk, phone);
  const pos = addr => desk.clients().find(c => c.address === addr).at.join(",");
  const [pa, pb] = [pos(a.address), pos(b.address)];
  const page = view.page, t = await touch(page);
  const ca = await centre(page, a.address), cb = await centre(page, b.address);

  await t.down(ca);
  await page.waitForTimeout(600);                                   // hold: lifts the window
  await expect(page.locator(".tile.lifted")).toBeVisible();
  // The outline really is that window: same box as its Hyprland geometry.
  const lift = await page.locator(".tile.lifted").boundingBox();
  expect(Math.abs(lift.x + lift.width / 2 - ca.x)).toBeLessThan(4);
  for (let i = 1; i <= 8; i++) await t.move({ x: ca.x + (cb.x - ca.x) * i / 8, y: ca.y + (cb.y - ca.y) * i / 8 });
  // A snapshot of the window rides under the finger.
  await expect(page.locator("#ghost.on")).toBeVisible();
  const g = await page.locator("#ghost").boundingBox();
  expect(Math.hypot(g.x + g.width / 2 - cb.x, g.y + g.height / 2 - cb.y), "ghost centred on the finger").toBeLessThan(6);
  await expect(page.locator(".tile.target")).toBeVisible();
  await expect(page.locator("#tile-hint")).toContainText("Let go to swap");
  await page.screenshot({ path: info.outputPath("dragging.png") });
  await t.up();

  await waitFor(() => pos(a.address) === pb && pos(b.address) === pa, "Hyprland swapped the two tiles");
  await expect(page.locator(".tile")).toHaveCount(0);
});

test("long-press a tile and close it", async ({ desk, phone }, info) => {
  const { a, b, view } = await twoTiles(desk, phone);
  const page = view.page, t = await touch(page);
  await t.down(await centre(page, b.address));
  await page.waitForTimeout(600);
  await t.up();
  // Let go without dragging: the action sheet slides up, big buttons.
  await expect(page.locator("#tile-sheet.up")).toBeVisible();
  const close = await page.locator("#tile-close").boundingBox();
  expect(close.height).toBeGreaterThanOrEqual(48);
  expect(close.width).toBeGreaterThan(page.viewportSize().width * 0.8);
  await page.waitForTimeout(250);
  await page.screenshot({ path: info.outputPath("sheet.png") });
  await page.tap("#tile-close");
  await waitFor(() => !desk.clients().some(c => c.address === b.address), "closed on the machine");
  expect(desk.clients().some(c => c.address === a.address), "the other window stays").toBe(true);
});

test("a tap elsewhere puts a lifted tile back without doing anything", async ({ desk, phone }) => {
  const { a, b, view } = await twoTiles(desk, phone);
  const before = desk.clients().filter(c => c.workspace.id === WS).map(c => c.address + c.at).sort();
  const page = view.page, t = await touch(page);
  await t.down(await centre(page, a.address));
  await page.waitForTimeout(600);
  await t.up();
  await expect(page.locator("#tile-sheet.up")).toBeVisible();
  await t.down(await centre(page, b.address)); await t.up();
  await expect(page.locator(".tile")).toHaveCount(0);
  await expect(page.locator("#tile-sheet.on")).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(desk.clients().filter(c => c.workspace.id === WS).map(c => c.address + c.at).sort()).toEqual(before);
});

test("a quick tap is still a click, not a lift", async ({ desk, phone }) => {
  const { a, view } = await twoTiles(desk, phone);
  const page = view.page, t = await touch(page);
  await t.down(await centre(page, a.address));
  await page.waitForTimeout(120);
  await t.up();
  await page.waitForTimeout(400);
  await expect(page.locator(".tile")).toHaveCount(0);
});

test("long-press, let go, and move the window to another workspace from the sheet", async ({ desk, phone }) => {
  const { a, view } = await twoTiles(desk, phone);
  const page = view.page, t = await touch(page);
  await t.down(await centre(page, a.address));
  await page.waitForTimeout(600);
  await t.up();
  await expect(page.locator("#tile-sheet.up")).toBeVisible();
  await page.tap('#tile-ws button[data-ws="9"]');
  await waitFor(() => desk.clients().find(c => c.address === a.address)?.workspace.id === 9, "window moved to 9");
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.close({ window = "address:${a.address}" })`));
});

test("a fullscreen window covers the others: nothing behind it can be lifted or swapped", async ({ desk, phone }) => {
  const { a, b, view } = await twoTiles(desk, phone);
  desk.dispatch(`hl.dsp.window.fullscreen({ window = "address:${a.address}" })`);
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.fullscreen({ window = "address:${a.address}" })`));
  await view.page.waitForFunction(x => state.windows.find(w => w.address === x)?.fullscreen, a.address, { timeout: 5000 });
  // Where b used to be is now the fullscreen window a.
  const page = view.page, t = await touch(page);
  const old = await page.evaluate(x => { const w = state.windows.find(w => w.address === x), r = tileRect(w), s = document.getElementById("stage").getBoundingClientRect(); return { x: s.left + r.x + r.w / 2, y: s.top + r.y + r.h / 2 }; }, b.address);
  expect(await page.evaluate(([x, y]) => windowAt(x, y)?.address, [old.x, old.y]), "the window under the finger is the fullscreen one").toBe(a.address);
  await t.down(old);
  await page.waitForTimeout(600);
  await t.up();
  await expect(page.locator("#tile-sheet.up")).toBeVisible();
  await expect(page.locator("#tile-title")).toContainText(a.class);
  // Its outline is the whole screen, not the window hidden behind it.
  const lift = await page.locator(".tile.lifted").boundingBox(), stage = await page.locator("#screen").boundingBox();
  expect(Math.abs(lift.width - stage.width)).toBeLessThan(6);
});

test("swapping three tiles in a row swaps exactly the ones you drag", async ({ desk, phone }) => {
  desk.dispatch(`hl.dsp.focus({ workspace = "${WS}" })`);
  const view = await phone.open(desk, { workspace: String(WS) });
  await waitFor(() => desk.monitors().find(m => isPhoneScreen(m))?.activeWorkspace.id === WS, "phone screen on the test workspace");
  const wins = [];
  for (let i = 0; i < 3; i++) wins.push(await desk.terminal("$HOME", WS));
  await view.page.waitForFunction(n => state.windows.filter(w => w.workspace === n).length >= 3, WS, { timeout: 15_000 });
  await view.page.waitForTimeout(400);
  const page = view.page, t = await touch(page);
  const slot = () => Object.fromEntries(desk.clients().filter(c => c.workspace.id === WS).map(c => [c.address, c.at.join(",")]));
  // Drag one window onto another, straight away again, and once more,
  // without waiting for the layout to settle between swaps.
  for (const [from, to] of [[0, 1], [1, 2], [2, 0]]) {
    const before = slot();
    const a = wins[from].address, b = wins[to].address;
    const ca = await centre(page, a), cb = await centre(page, b);
    await t.down(ca); await page.waitForTimeout(500);
    for (let i = 1; i <= 6; i++) await t.move({ x: ca.x + (cb.x - ca.x) * i / 6, y: ca.y + (cb.y - ca.y) * i / 6 });
    await t.up();
    await waitFor(() => { const s = slot(); return s[a] === before[b] && s[b] === before[a]; }, `swapped ${from}<->${to} and only those`);
    const after = slot();
    for (const w of wins) if (w.address !== a && w.address !== b) expect(after[w.address], "a third window didn't move").toBe(before[w.address]);
  }
});

test("taps on a fullscreen window go to it, not the windows behind", async ({ desk, phone }) => {
  const { a, b, view } = await twoTiles(desk, phone);
  desk.dispatch(`hl.dsp.window.fullscreen({ window = "address:${a.address}" })`);
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.fullscreen({ window = "address:${a.address}" })`));
  await view.page.waitForFunction(x => state.windows.find(w => w.address === x)?.fullscreen, a.address, { timeout: 5000 });
  // Tap where b sits under the fullscreen window: focus must stay on a.
  const old = await view.page.evaluate(x => { const w = state.windows.find(w => w.address === x), r = tileRect(w), s = document.getElementById("stage").getBoundingClientRect(); return { x: s.left + r.x + r.w / 2, y: s.top + r.y + r.h / 2 }; }, b.address);
  const t = await touch(view.page);
  await t.down(old); await view.page.waitForTimeout(80); await t.up();
  await view.page.waitForTimeout(500);
  expect(JSON.parse(desk.sh("hyprctl -j activewindow")).address).toBe(a.address);
});

test("quick taps (like typing fast or a double-click) never zoom the view", async ({ desk, phone }) => {
  const { a, view } = await twoTiles(desk, phone);
  const page = view.page, t = await touch(page);
  const zoom = () => page.evaluate(() => [view.s, view.x, view.y].map(n => Math.round(n * 1000)));
  const before = await zoom();
  const at = await centre(page, a.address);
  // Check after every tap: an old double-tap zoom went in on the 2nd tap and
  // back out on the 4th, which a single check at the end would miss.
  for (let i = 0; i < 4; i++) {
    await t.down(at); await page.waitForTimeout(40); await t.up(); await page.waitForTimeout(90);
    expect(await zoom(), `zoom after tap ${i + 1}`).toEqual(before);
  }
});
