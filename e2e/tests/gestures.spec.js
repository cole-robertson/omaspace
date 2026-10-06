// Phone gestures and voice: swipe sideways between workspaces (only a real
// swipe; taps and slow drags stay mouse input), and hold-to-talk dictation
// through Omarchy's voxtype on the watched machine.

import { test, expect, waitFor } from "../lib/fixtures.js";

const WS = 5;

test.beforeEach(async ({ desk }) => {
  desk.dispatch(`hl.dsp.focus({ workspace = "${WS}" })`);
});

/** A one-finger touch drag across the stage, as CDP touch events. */
async function drag(page, from, to, ms, steps = 12) {
  const cdp = await page.context().newCDPSession(page);
  const box = await page.locator("#stage").boundingBox();
  const at = ([fx, fy]) => ({ x: box.x + fx * box.width, y: box.y + fy * box.height });
  const a = at(from), b = at(to);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [a] });
  for (let i = 1; i <= steps; i++) {
    const p = { x: a.x + (b.x - a.x) * i / steps, y: a.y + (b.y - a.y) * i / steps };
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [p] });
    await new Promise(r => setTimeout(r, ms / steps));
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

test("a quick swipe left goes to the next workspace, right goes back", async ({ desk, phone }) => {
  const view = await phone.open(desk, { workspace: String(WS) });
  const phoneWs = () => desk.monitors().find(m => m.name.startsWith("OSP-PHONE"))?.activeWorkspace.id;
  await waitFor(() => phoneWs() === WS, "phone screen on the start workspace");
  await drag(view.page, [0.85, 0.5], [0.15, 0.52], 180);
  await waitFor(() => phoneWs() === WS + 1, `swipe left → workspace ${WS + 1}`);
  await expect.poll(() => view.activeWorkspace()).toBe(WS + 1);
  await drag(view.page, [0.15, 0.5], [0.85, 0.5], 180);
  await waitFor(() => phoneWs() === WS, `swipe right → back to ${WS}`);
});

test("a slow sideways drag or a mostly vertical one is not a swipe", async ({ desk, phone }) => {
  const view = await phone.open(desk, { workspace: String(WS) });
  const phoneWs = () => desk.monitors().find(m => m.name.startsWith("OSP-PHONE"))?.activeWorkspace.id;
  await waitFor(() => phoneWs() === WS, "phone screen on the start workspace");
  await drag(view.page, [0.6, 0.5], [0.4, 0.5], 1500);      // short and slow: a cursor drag
  await drag(view.page, [0.5, 0.3], [0.35, 0.8], 200);      // fast but diagonal/vertical: a scroll
  await new Promise(r => setTimeout(r, 800));
  expect(phoneWs()).toBe(WS);
  expect(await view.page.evaluate(() => lastSwipe)).toBeNull();
});

test("there is no workspace 0: swiping right on 1 springs back", async ({ desk, phone }) => {
  desk.dispatch(`hl.dsp.focus({ workspace = "1" })`);
  const view = await phone.open(desk, { workspace: "1" });
  await drag(view.page, [0.15, 0.5], [0.85, 0.5], 180);
  const swipe = await view.page.evaluate(() => lastSwipe);
  expect(swipe.target).toBeNull();
  await new Promise(r => setTimeout(r, 400));
  expect(await view.page.evaluate(() => getComputedStyle(document.getElementById("screen")).translate)).toMatch(/^(none|0px)/);
});

test("keyboard dictation: a draft that rewrites itself lands once, as the final text", async ({ desk, phone }) => {
  const dir = desk.scratch("dict");
  const term = await desk.terminal(dir, WS);
  const view = await phone.open(desk, { workspace: String(WS) });
  await view.pointAt(0.5, 0.2);
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  // What iOS dictation does to the field while you talk.
  await view.dictate(["echo hell", "echo hello word", "echo hello world", "echo Hello, world > said.txt"]);
  await view.page.locator("#kbd").press("Enter");
  await waitFor(() => desk.exists(`${dir}/said.txt`), "the dictated command ran");
  expect(desk.read(`${dir}/said.txt`)).toBe("Hello, world");
});

test("phones dictate with the keyboard's mic: no hold-to-talk button there", async ({ desk, phone, laptop }) => {
  const p = await phone.open(desk, {});
  await expect(p.page.locator("#mic-btn")).toBeHidden();
  const l = await laptop.open(desk);
  await expect(l.page.locator("#mic-btn")).toBeVisible();
});

test("hold to talk (laptop): speech is transcribed on the machine and typed into the terminal", async ({ desk, talker }) => {
  const dir = desk.scratch("talk");
  const term = await desk.terminal(dir, WS);
  const view = await talker.open(desk);
  // Laptop view shows the real monitor: focus the terminal there.
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  await waitFor(() => desk.activeWorkspace() === WS, "terminal's workspace showing");
  // The fake microphone loops the clip (0.3s silence, ~2.3s speech, 0.6s
  // silence); holding two passes always contains one whole phrase.
  const mic = view.page.locator("#mic-btn"), box = await mic.boundingBox();
  await view.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await view.page.mouse.down();
  await expect(view.page.locator("#talk.on")).toContainText("listening");
  await new Promise(r => setTimeout(r, 6400));
  await view.page.mouse.up();
  await expect(view.page.locator("#talk .txt")).toContainText(/spoken/i, { timeout: 20_000 });
  // The text was typed at the prompt: turn the line into `printf %s '…' > said.txt`
  // (Home, then type around it) so the shell writes what was dictated.
  await view.page.evaluate(() => send({ type: "combo", code: "Home", mods: 0 }));
  await view.type("printf '%s' '");
  await view.page.evaluate(() => send({ type: "combo", code: "End", mods: 0 }));
  await view.type("' > said.txt\n");
  await waitFor(() => desk.exists(`${dir}/said.txt`), "the dictated line ran");
  expect(desk.read(`${dir}/said.txt`).toLowerCase()).toContain("spoken");
});

test("dictation refuses anything that isn't a WAV clip", async ({ desk }) => {
  const r = await fetch(`${desk.viewUrl()}/v1/view/dictate`, { method: "POST", body: "not audio" });
  expect((await r.json()).error).toMatch(/not a WAV/);
});

test("switching workspaces never decodes on top of a missing frame (no smearing)", async ({ desk, phone }) => {
  const view = await phone.open(desk, { workspace: String(WS) });
  // Watch what reaches the decoder: after a dropped frame, only a keyframe may come next.
  await view.page.evaluate(() => {
    window.__bad = 0; window.__keys = 0;
    const real = VideoDecoder.prototype.decode;
    let gap = false, lastDropped = 0;
    // A slow phone: pretend the decoder is backed up every 3rd delta frame.
    let n = 0;
    Object.defineProperty(VideoDecoder.prototype, "decodeQueueSize", { get() { return ++n % 3 === 0 ? 5 : 0; } });
    VideoDecoder.prototype.decode = function (chunk) {
      if (dropped > lastDropped) { gap = true; lastDropped = dropped; }
      if (chunk.type === "key") { gap = false; window.__keys++; }
      else if (gap) window.__bad++;
      return real.call(this, chunk);
    };
  });
  // Swipe back and forth fast, the way that smeared on a phone.
  for (let i = 0; i < 6; i++) await drag(view.page, i % 2 ? [0.15, 0.5] : [0.85, 0.5], i % 2 ? [0.85, 0.5] : [0.15, 0.5], 150);
  await new Promise(r => setTimeout(r, 1500));
  const r = await view.page.evaluate(() => ({ bad: window.__bad, keys: window.__keys, dropped }));
  expect(r.dropped, "the slow-phone simulation dropped frames").toBeGreaterThan(0);
  expect(r.bad, `delta frames decoded after a drop (${r.dropped} dropped)`).toBe(0);
  expect(r.keys, "keyframes arrive often (every 0.5s)").toBeGreaterThanOrEqual(3);
  // And no sliding: the picture itself never moves during a swipe.
  expect(await view.page.evaluate(() => getComputedStyle(document.getElementById("screen")).translate)).toMatch(/^(none|0px)/);
});

test("arrow pad: hold and slide up walks back through shell history; a tap is one arrow", async ({ desk, phone }) => {
  const dir = desk.scratch("arrows");
  const term = await desk.terminal(dir, WS);
  const view = await phone.open(desk, { workspace: String(WS) });
  await view.pointAt(0.5, 0.2);
  desk.dispatch(`hl.dsp.focus({ window = "address:${term.address}" })`);
  // History: three commands.
  for (const n of [1, 2, 3]) await view.type(`echo h${n} > /dev/null\n`);
  const page = view.page, pad = await page.locator("#arrows").boundingBox();
  const cx = pad.x + pad.width / 2, cy = pad.y + pad.height / 2;
  const sent = [];
  await page.exposeFunction("__sent", m => sent.push(m));
  await page.evaluate(() => { const real = ws.send.bind(ws); ws.send = d => { const m = JSON.parse(d); if (m.type === "combo") window.__sent(m.code); return real(d); }; });
  // Tap the top edge: one Up.
  await page.mouse.move(cx, pad.y + 4); await page.mouse.down(); await page.mouse.up();
  await page.waitForTimeout(200);
  expect(sent).toEqual(["ArrowUp"]);
  // Hold and slide up: Up repeats while held.
  sent.length = 0;
  await page.mouse.move(cx, cy); await page.mouse.down();
  await page.mouse.move(cx, cy - 40, { steps: 4 });
  await page.waitForTimeout(700);
  await page.mouse.up();
  const ups = sent.filter(c => c === "ArrowUp").length;
  expect(ups, "held slide repeats").toBeGreaterThanOrEqual(3);
  expect(sent.every(c => c === "ArrowUp")).toBe(true);
  // A fresh prompt (Ctrl+C), then one Up from the pad recalls the newest
  // command (Omarchy binds Up to history-search-backward, so the line must
  // be empty); edit its target and run it: the file says which came back.
  await page.waitForTimeout(400);
  await page.evaluate(() => send({ type: "combo", code: "KeyC", mods: 4 }));
  await page.waitForTimeout(300);
  await page.mouse.move(cx, pad.y + 4); await page.mouse.down(); await page.mouse.up();
  await page.waitForTimeout(200);
  for (let i = 0; i < "/dev/null".length; i++) await page.evaluate(() => send({ type: "combo", code: "Backspace", mods: 0 }));
  await view.type("got.txt\n");
  await waitFor(() => desk.exists(`${dir}/got.txt`), "a recalled history line ran");
  expect(desk.read(`${dir}/got.txt`)).toBe("h3");
});

test("arrow pad slides never swipe workspaces or lift tiles", async ({ desk, phone }) => {
  const view = await phone.open(desk, { workspace: String(WS) });
  const page = view.page, pad = await page.locator("#arrows").boundingBox();
  const before = desk.monitors().find(m => m.name.startsWith("OSP-PHONE"))?.activeWorkspace.id;
  await page.mouse.move(pad.x + pad.width / 2, pad.y + pad.height / 2); await page.mouse.down();
  await page.mouse.move(pad.x + pad.width / 2 + 120, pad.y + pad.height / 2, { steps: 6 });
  await page.waitForTimeout(400); await page.mouse.up();
  expect(await page.evaluate(() => [lastSwipe, !!lifted])).toEqual([null, false]);
  expect(desk.monitors().find(m => m.name.startsWith("OSP-PHONE"))?.activeWorkspace.id).toBe(before);
});
