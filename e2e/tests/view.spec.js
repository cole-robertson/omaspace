// The live view: laptop and phone, controls, phone mode, files, agents.

import { test, expect, waitFor } from "../lib/fixtures.js";

const WS = 7;

test.beforeEach(async ({ desk }) => {
  await desk.clearWorkspace(WS);
  // Leftover notifications sit over the windows and would catch taps.
  desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
});

test("laptop: streams at native size and types into a terminal", async ({ desk, laptop }) => {
  const dir = desk.scratch("view");
  const term = await desk.terminal(dir, WS);
  const view = await laptop.open(desk);
  // Static frames are skipped, so an idle screen streams only a few fps.
  const s = await view.stats();
  expect(s.fps).toBeGreaterThan(0);
  const mon = desk.monitors().find(m => !m.name.startsWith("OSP-"));
  expect([s.width, s.height]).toEqual([mon.width, mon.height]);

  await view.tapWorkspace(WS);
  await waitFor(() => desk.activeWorkspace() === WS, "desk on the test workspace");
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  await view.pointAt(term.at[0] / mon.width + 0.05, term.at[1] / mon.height + 0.05);
  await view.type('echo "typed: ok" > typed.txt\n');
  await waitFor(() => desk.exists(`${dir}/typed.txt`), "typed.txt");
  expect(desk.read(`${dir}/typed.txt`)).toBe("typed: ok");
});

test("laptop: SUPER+number from the key bar switches the real workspace", async ({ desk, laptop }) => {
  const view = await laptop.open(desk);
  await view.combo(["SUPER"], `Digit${WS}`);
  await waitFor(() => desk.activeWorkspace() === WS, `SUPER+${WS} switched desk`);
});

test("phone: a phone-shaped virtual screen, workspace strip, and cleanup", async ({ desk, phone }) => {
  await desk.terminal("$HOME", WS);
  const view = await phone.open(desk, { workspace: String(WS) });
  const phoneOut = await waitFor(() => desk.monitors().find(m => m.name.startsWith("OSP-PHONE")), "phone screen");
  expect(phoneOut.height).toBeGreaterThan(phoneOut.width);
  expect(phoneOut.activeWorkspace.id).toBe(WS);
  const s = await view.stats();
  expect([s.width, s.height]).toEqual([phoneOut.width, phoneOut.height]);
  expect(desk.sh("hyprctl configerrors")).toBe("");

  await view.tapWorkspace(8);
  await waitFor(() => desk.monitors().find(m => m.name === phoneOut.name)?.activeWorkspace.id === 8, "phone screen shows ws8");
  await expect.poll(() => view.activeWorkspace(), { message: "strip follows the phone screen" }).toBe(8);
  await view.tapWorkspace(WS);
  await waitFor(() => desk.monitors().find(m => m.name === phoneOut.name)?.activeWorkspace.id === WS, "phone screen back on ws");

  await view.close();
  await waitFor(() => !desk.monitors().some(m => m.name.startsWith("OSP-PHONE")), "phone screen removed on disconnect");
});

test("phone: typing on the phone keyboard runs in the terminal", async ({ desk, phone }) => {
  const dir = desk.scratch("phone");
  const term = await desk.terminal(dir, WS);
  const view = await phone.open(desk, { workspace: String(WS) });
  await view.pointAt(0.5, 0.25);
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  await view.type('echo "From my Phone!" > p.txt\n');
  await waitFor(() => desk.exists(`${dir}/p.txt`), "p.txt written from the phone");
  expect(desk.read(`${dir}/p.txt`)).toBe("From my Phone!");
});

test("window panel: float and move a window to another workspace", async ({ desk, laptop }) => {
  const win = await desk.terminal("$HOME", WS);
  const view = await laptop.open(desk);
  await view.openWindows();
  const card = view.page.locator(`.win[data-address="${win.address}"]`);
  await card.locator("button", { hasText: "Float" }).click();
  await waitFor(() => desk.clients().find(c => c.address === win.address)?.floating, "window floating");
  await card.locator("select").selectOption("9");
  await waitFor(() => desk.clients().find(c => c.address === win.address)?.workspace.id === 9, "window on ws9");
});

test("files: phone upload, listing, download, and desktop drag and drop", async ({ desk, peer, phone, laptop }, info) => {
  const local = info.outputPath("photo.jpg");
  peer.randomFile(local, 6_000_000);
  desk.onCleanup(() => desk.rm("$HOME/Downloads/photo.jpg"));
  desk.onCleanup(() => desk.rm("$HOME/Downloads/dropped.txt"));

  const p = await phone.open(desk, { mode: "desktop" });
  await p.openFiles();
  expect((await p.upload([local]))[0]).toMatch(/^✓ ~\/Downloads\/photo\.jpg/);
  expect(desk.sha256("$HOME/Downloads/photo.jpg")).toBe(peer.sha256(local));
  await waitFor(async () => (await p.fileNames()).includes("photo.jpg"), "photo listed");
  const saved = await p.download("photo.jpg", info.outputPath("back.jpg"));
  expect(peer.sha256(saved)).toBe(peer.sha256(local));

  const l = await laptop.open(desk);
  expect((await l.drop([{ name: "dropped.txt", content: "from a laptop\n" }]))[0]).toMatch(/^✓/);
  expect(desk.read("$HOME/Downloads/dropped.txt")).toBe("from a laptop");
});

test("agent: cursor, help request, take over pauses the agent, hand back resumes it", async ({ desk, laptop }) => {
  const view = await laptop.open(desk);
  const agent = body => fetch(`${desk.viewUrl()}/v1/view/agent`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).then(r => r.json());

  expect(await agent({ name: "TestAgent", x: 0.4, y: 0.3 })).toEqual({ ok: true });
  await expect(view.page.locator(".cursor.agent", { hasText: "TestAgent" })).toBeVisible();
  await agent({ name: "TestAgent", help: "need a 2FA code" });
  await expect(view.page.locator("#banner.on")).toContainText("need a 2FA code");
  await view.page.click("#takeover");
  // The take-over goes over the websocket; wait for the server's broadcast.
  await expect(view.page.locator("#handback")).toBeVisible();
  expect(await agent({ name: "TestAgent", x: 0.5, y: 0.5 })).toMatchObject({ ok: false, paused: true });
  desk.onCleanup(() => desk.sh("omarchy-shell -q notifications dismissAll", { check: false }));
  await view.page.click("#handback");
  await expect(view.page.locator("#handback")).toBeHidden();
  expect(await agent({ name: "TestAgent", x: 0.5, y: 0.5 })).toEqual({ ok: true });
});

test("security: the view refuses connections that don't come through Tailscale", async ({ desk }) => {
  // No TCP listener at all: other users on the machine have nothing to forge headers to.
  expect(desk.sh("ss -ltnH 'sport = :7788' | grep -c '127.0.0.1' || true").trim()).toBe("0");
  const sock = desk.sh("echo ${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/omaspace-view.sock").trim();
  expect(desk.sh(`stat -c %a ${sock}`).trim()).toBe("600");
  expect(desk.sh(`curl -s -o /dev/null -w '%{http_code}' --unix-socket ${sock} http://x/`)).toBe("403");
  expect(desk.sh(`curl -s -o /dev/null -w '%{http_code}' --unix-socket ${sock} -H 'X-Forwarded-For: 8.8.8.8' http://x/`)).toBe("403");
});

test("security: other web sites cannot open the view", async ({ desk }) => {
  const ws = `curl -s -m 4 -o /dev/null -w '%{http_code}' --http1.1 -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ=='`;
  expect(desk.sh(`${ws} -H 'Origin: https://evil.example' '${desk.viewUrl()}/ws?w=320' || true`)).toBe("403");
  expect(desk.sh(`curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Origin: https://evil.example' -H 'content-type: text/plain' --data '{"help":"x"}' '${desk.viewUrl()}/v1/view/agent'`)).toBe("403");
  expect(desk.sh(`curl -s -D - -o /dev/null -H 'Origin: https://evil.example' '${desk.viewUrl()}/v1/view/presence'`)).not.toMatch(/access-control-allow-origin/i);
});

test("phone: opening the keyboard keeps the spot you tapped in view", async ({ desk, phone }) => {
  const dir = desk.scratch("kbdview");
  await desk.terminal(dir, WS);
  const view = await phone.open(desk, { workspace: String(WS) });
  const page = view.page;
  // The prompt is at the top of the terminal; tap there.
  await view.pointAt(0.3, 0.08);
  await page.tap("#kbd-btn");
  // iOS: the keyboard shrinks the visual viewport to the part above it.
  const full = await page.evaluate(() => innerHeight);
  await page.setViewportSize({ width: page.viewportSize().width, height: Math.round(full * 0.5) });
  await page.evaluate(() => window.visualViewport.dispatchEvent(new Event("resize")));
  await expect(page.locator("body.kbd-up")).toBeAttached();
  // Settled: the stream is at least full width (it may still be resizing to the phone screen for a frame).
  await page.waitForFunction(() => {
    const c = document.getElementById("screen"), m = new DOMMatrix(getComputedStyle(c).transform);
    return m.a >= document.getElementById("stage").clientWidth / c.width - 1e-3;
  });
  const where = await page.evaluate(() => {
    const st = document.getElementById("stage").getBoundingClientRect();
    const c = document.getElementById("screen"), m = new DOMMatrix(getComputedStyle(c).transform);
    return { tapY: m.f + 0.08 * c.height * m.d, stageH: st.height, stageTop: st.top, keyBar: document.querySelector("footer").getBoundingClientRect().bottom, vv: visualViewport.height, scale: m.a, fit: st.width / c.width };
  });
  expect(where.tapY, "tapped point is inside the visible stage").toBeGreaterThan(0);
  expect(where.tapY).toBeLessThan(where.stageH);
  expect(where.keyBar, "key bar sits right above the keyboard").toBeLessThanOrEqual(where.vv + 1);
  expect(where.scale, "stream stays full width, not shrunk").toBeGreaterThanOrEqual(where.fit - 1e-3);
  await view.type('echo "seen" > s.txt\n');
  await waitFor(() => desk.exists(`${dir}/s.txt`), "typing still reaches the terminal");
});

test("phone: the window list stays live while the phone is streaming", async ({ desk, phone }) => {
  // While a screen is captured Hyprland sends an event every frame; updates
  // must still go out (they once waited for a quiet socket that never came).
  desk.dispatch(`hl.dsp.focus({ workspace = "${WS}" })`);
  const view = await phone.open(desk, { workspace: String(WS) });
  await view.page.waitForTimeout(1000);
  const win = await desk.terminal("$HOME", WS);
  await view.page.waitForFunction(a => state.windows.some(w => w.address === a), win.address, { timeout: 3000 });
  desk.dispatch(`hl.dsp.window.close({ window = "address:${win.address}" })`);
  await view.page.waitForFunction(a => !state.windows.some(w => w.address === a), win.address, { timeout: 3000 });
});
