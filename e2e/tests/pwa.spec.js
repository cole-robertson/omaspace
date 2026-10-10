// Add to home screen, and switching between your machines.

import { test, expect, waitFor } from "../lib/fixtures.js";
import { isPhoneScreen } from "../lib/machine.js";

test("installable: manifest, theme colors, and icons that render", async ({ desk, phone }) => {
  const view = await phone.open(desk, {});
  const m = await (await fetch(`${desk.viewUrl()}/manifest.webmanifest`)).json();
  expect(m.display).toBe("standalone");
  expect(m.start_url).toBe("/");
  expect(m.short_name).toBe(desk.name);
  expect(m.icons.map(i => i.type)).toEqual(expect.arrayContaining(["image/svg+xml", "image/png"]));
  const png = await fetch(`${desk.viewUrl()}/icon.png`);
  expect(png.status).toBe(200);
  expect(new Uint8Array(await png.arrayBuffer()).slice(1, 4)).toEqual(new Uint8Array([0x50, 0x4e, 0x47]));
  const accent = desk.sh("grep '^accent' ~/.local/state/omarchy/current/theme/colors.toml | cut -d'\"' -f2");
  expect(await (await fetch(`${desk.viewUrl()}/icon.svg`)).text()).toContain(accent);
  // iOS reads these tags for a home-screen app.
  for (const sel of ['link[rel="apple-touch-icon"]', 'meta[name="apple-mobile-web-app-capable"]', 'link[rel="manifest"]'])
    await expect(view.page.locator(sel)).toHaveCount(1);
});

test("the machine switcher lists your machines, who's watching and agents working", async ({ desk, peer, laptop }, info) => {
  const view = await laptop.open(desk);
  // An agent reports in, so the switcher should show it on this machine.
  await fetch(`${desk.viewUrl()}/v1/view/agent`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "Builder", x: 0.5, y: 0.5 }) });
  await view.page.click("#host");
  const here = view.page.locator(`#machines .m[data-name="${desk.name}"]`);
  await expect(here).toHaveClass(/here/);
  await expect(here.locator(".who")).toContainText("1");
  await expect(here.locator(".who .ag")).toContainText("1");
  await expect(view.page.locator(`#machines .m[data-name="${peer.name}"]`)).toBeVisible();
  await view.page.screenshot({ path: info.outputPath("machines.png") });
});

test("when the view changes shape (home-screen app, rotation) the phone's screen follows: no black bars", async ({ desk, phone }) => {
  desk.dispatch(`hl.dsp.focus({ workspace = "6" })`);
  const view = await phone.open(desk, { workspace: "6" });
  const page = view.page;
  // Other phones may be watching too: find this page's screen by its size.
  const shape = () => page.evaluate(() => phoneShape.split("x").map(Number));
  const mine = async () => { const [w, h] = await shape(); return desk.monitors().find(m => isPhoneScreen(m) && Math.abs(m.height - h) < 8 && Math.abs(m.width - w) < 8); };
  const first = await waitFor(mine, "phone screen");
  // A home-screen app is taller than a Safari tab.
  const vp = page.viewportSize();
  await page.setViewportSize({ width: vp.width, height: vp.height + 120 });
  const taller = await waitFor(async () => { const s = await mine(); return s && s.height > first.height + 100 && s; }, "a taller phone screen", 15_000);
  // The stream fills the stage edge to edge, no bars.
  await page.waitForFunction(() => {
    const c = document.getElementById("screen"), st = document.getElementById("stage");
    const m = new DOMMatrix(getComputedStyle(c).transform), w = c.width * m.a, h = c.height * m.d;
    return c.width > 0 && Math.abs(w - st.clientWidth) < 3 && Math.abs(h - st.clientHeight) < 3;
  }, null, { timeout: 15_000 });
  // Still on the same workspace, and the old screen is gone.
  expect(taller.activeWorkspace.id).toBe(6);
  await waitFor(() => !desk.monitors().some(m => m.name === first.name), "old phone screen removed");
});

test("home-screen app fills the screen: no strip below the dock", async ({ desk, phone }) => {
  const view = await phone.open(desk, {});
  await view.page.emulateMedia({ media: "screen" });
  const css = await view.page.evaluate(() => [...document.styleSheets[0].cssRules].map(r => r.cssText).join("\\n"));
  expect(css).toMatch(/display-mode: standalone/);
});
