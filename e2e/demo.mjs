// Records the omaspace demo: the live view in a laptop browser, then on a
// phone, with an agent working next to you. Each scene is captured as a
// stream of screen frames (CDP screencast) and its timeline written as JSON;
// demo-cut.sh turns them into one captioned video.
//
//   OMASPACE_E2E_DESK=host OUT=/tmp/demo node demo.mjs
//
// Uses workspaces 7-9 on the desk and a throwaway browser profile; closes
// everything it opens.
import { chromium, devices } from "@playwright/test";
import fs from "node:fs";
import { Machine, waitFor, shq } from "./lib/machine.js";

const desk = new Machine(process.env.OMASPACE_E2E_DESK);
const OUT = process.env.OUT || "/tmp/demo";
fs.rmSync(OUT, { recursive: true, force: true });
fs.mkdirSync(OUT, { recursive: true });
const sleep = ms => new Promise(r => setTimeout(r, ms));
const DEMO = `/tmp/omaspace-demo`;
const AGENT = "Claude";
// A second machine to switch to from the phone (the live view's machine list).
const PEER = process.env.OMASPACE_E2E_PEER;
const peer = PEER ? new Machine(PEER) : null;
// Windows the Spaces drop gives to the peer's workspace 4 are closed after.
const peerWsBefore = peer ? new Set(peer.windowsOn(4).map(c => c.address)) : new Set();
let peerBefore = new Set(), arrived = null;

// ---- staging --------------------------------------------------------------
fs.rmSync(DEMO, { recursive: true, force: true });
fs.mkdirSync(`${DEMO}/trip`, { recursive: true });
fs.writeFileSync(`${DEMO}/trip/packing.md`, `# Packing

- [ ] Passport
- [ ] Charger, adapter
- [ ] Walking shoes
- [ ] Rain jacket (just in case)
`);
fs.writeFileSync(`${DEMO}/trip/plan.md`, `# Lisbon, Fri to Sun

- [x] Flights booked
- [ ] Dinner Fri, 7:30?
- [ ] Bikes Sat morning
- [ ] Tram 28 to the castle
`);
// Workspaces 4, 5, 7, 8, 9 are the demo's; 6 (and 1-3) are left alone.
for (const ws of [4, 5, 7, 8, 9]) await desk.clearWorkspace(ws).catch(() => {});
const opened = [];
const open = async (cmd, ws, match) => { const w = await desk.open(cmd, ws, { match }); opened.push(w); return w; };
const term = (dir, ws, run) => open(`uwsm-app -- xdg-terminal-exec --dir=${dir}${run ? ` -e ${run}` : ""}`, ws, c => /foot|terminal|alacritty|ghostty|kitty/i.test(c.class));
// The phone's terminals: Omarchy's own terminal and font, as on a real phone.
const bigTerm = (dir, ws, run) => open(`uwsm-app -- foot --working-directory=${dir}${run ? ` ${run}` : ""}`, ws, c => c.class === "foot");
const chrome = (url, ws) => open(`uwsm-app -- chromium --user-data-dir=${DEMO}/chrome --no-first-run --no-default-browser-check --new-window ${url}`, ws, c => c.class === "chromium");

// Phone: workspace 5 has the plan and a terminal in a big font; swiping
// left lands on 4, the packing list.
await bigTerm(`${DEMO}/trip`, 4, "nvim -n packing.md");
await sleep(700);
const pEditor = await bigTerm(`${DEMO}/trip`, 5, "nvim -n plan.md");
await sleep(700);
const pShell = await bigTerm(`${DEMO}/trip`, 5, "env PS1='trip $ ' bash --norc --noprofile");
await sleep(700);

desk.dispatch(`hl.dsp.focus({ workspace = "7" })`);
const demoBrowser = await chrome("https://omarchy.org", 7);
await sleep(2500);
const editor = await term(`${DEMO}/trip`, 7, "nvim -n + plan.md");
await sleep(800);
const shell = await term(`${DEMO}/trip`, 7);
desk.typeInTerminal(shell, "clear; fastfetch --logo small");
await sleep(2500);
desk.sh("omarchy-shell -q notifications dismissAll", { check: false });

// The demo bank the agent signs in to, served locally (not a real bank).
const http = await import("node:http");
const page = fs.readFileSync(new URL("./demo-site/index.html", import.meta.url));
const site = http.createServer((_, res) => { res.writeHead(200, { "content-type": "text/html; charset=utf-8" }); res.end(page); });
await new Promise(r => site.listen(8765, "127.0.0.1", r));
const BANK = "http://127.0.0.1:8765/";

// ---- capture --------------------------------------------------------------
const browser = await chromium.launch({ executablePath: process.env.OMASPACE_E2E_CHROMIUM || "/usr/bin/chromium", args: ["--ozone-platform=headless"] });

/** One scene: a browser context recording its own frames and caption marks. */
async function scene(name, ctxOpts, size) {
  const ctx = await browser.newContext(ctxOpts);
  const page = await ctx.newPage();
  const cdp = await ctx.newCDPSession(page);
  const dir = `${OUT}/${name}`;
  fs.mkdirSync(dir);
  const frames = [], marks = [];
  let t0 = null;
  cdp.on("Page.screencastFrame", async f => {
    const t = f.metadata.timestamp * 1000;
    t0 ??= t;
    const file = `${dir}/${String(frames.length).padStart(5, "0")}.jpg`;
    fs.writeFileSync(file, Buffer.from(f.data, "base64"));
    frames.push({ file, t: t - t0 });
    await cdp.send("Page.screencastFrameAck", { sessionId: f.sessionId }).catch(() => {});
  });
  // The screencast captures at CSS pixels, a third of a 3x phone's real
  // resolution. For a high-DPI device, grab full-resolution screenshots in a
  // loop instead (about 30 a second), so the frames are sharp at full size.
  const dpr = ctxOpts.deviceScaleFactor || 1;
  let grabbing = false, grabLoop = null;
  const start = async () => {
    if (dpr <= 1) return cdp.send("Page.startScreencast", { format: "jpeg", quality: 88, maxWidth: size.w, maxHeight: size.h });
    grabbing = true;
    const t0 = Date.now();
    grabLoop = (async () => {
      while (grabbing) {
        const vp = ctxOpts.viewport;
        const r = await cdp.send("Page.captureScreenshot", { format: "jpeg", quality: 80, optimizeForSpeed: true, clip: { x: 0, y: 0, width: vp.width, height: vp.height, scale: Math.min(dpr, 2) } }).catch(() => null);
        if (!r) break;
        const file = `${dir}/${String(frames.length).padStart(5, "0")}.jpg`;
        fs.writeFileSync(file, Buffer.from(r.data, "base64"));
        frames.push({ file, t: Date.now() - t0 });
      }
    })();
  };
  const caption = text => marks.push({ t: frames.length ? frames[frames.length - 1].t : 0, text });
  // Fingers: every touch the page sees, timed on the frames' clock.
  const touches = [];
  const touchAt = (phase, x, y) => touches.push({ t: frames.length ? frames[frames.length - 1].t : 0, phase, x, y });
  const done = async () => {
    grabbing = false; await grabLoop;
    await cdp.send("Page.stopScreencast").catch(() => {});
    fs.writeFileSync(`${dir}/timeline.json`, JSON.stringify({ frames, marks, size, touches, viewport: ctxOpts.viewport }));
    await ctx.close();
  };
  return { ctx, page, cdp, start, caption, done, touchAt };
}
async function openView(page, params) {
  await page.goto(`${desk.viewUrl()}/?${new URLSearchParams({ name: "You", ...params })}`);
  await page.waitForFunction(() => /[1-9]\d* fps/.test(document.getElementById("stats")?.textContent || ""), null, { timeout: 30_000 });
  await sleep(1500);
}
const touch = (cdp, s) => {
  let last = null;
  return {
    down: p => { last = p; s?.touchAt("down", p.x, p.y); return cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [p] }); },
    move: p => { last = p; s?.touchAt("move", p.x, p.y); return cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [p] }); },
    up: () => { if (last) s?.touchAt("up", last.x, last.y); return cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] }); },
  };
};
/** Tap a locator like a finger: logged for the ripple, then tapped. */
async function tapOn(s, loc) {
  const b = await loc.boundingBox();
  if (b) { s.touchAt("down", b.x + b.width / 2, b.y + b.height / 2); s.touchAt("up", b.x + b.width / 2, b.y + b.height / 2); }
  await loc.tap();
}
const centre = (page, a) => page.evaluate(a => { const w = state.windows.find(x => x.address === a), t = tileRect(w), r = document.getElementById("stage").getBoundingClientRect(); return { x: r.left + t.x + t.w / 2, y: r.top + t.y + t.h / 2 }; }, a);
/** Type like a person, a key at a time, through the view's virtual keyboard. */
async function typeSlowly(page, text, gap = 55) {
  for (const ch of text) {
    // Shift is a real key on the virtual keyboard: hold it for capitals.
    if (/[A-Z]/.test(ch)) { await page.keyboard.down("Shift"); await page.keyboard.press(`Key${ch}`); await page.keyboard.up("Shift"); }
    else if (ch === " ") await page.keyboard.press("Space");
    else if (ch === "-") await page.keyboard.press("Minus");
    else if (ch === "[") await page.keyboard.press("BracketLeft");
    else if (ch === "]") await page.keyboard.press("BracketRight");
    else await page.keyboard.type(ch);
    await sleep(gap + Math.random() * 40);
  }
}

try {
  // ---- 1. Laptop: your desktop in a browser tab -----------------------------
  {
    const s = await scene("laptop", { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 }, { w: 1440, h: 900 });
    await openView(s.page, {});
    await s.start();
    s.caption("Your Omarchy desktop, live in any browser on your tailnet");
    await sleep(3500);
    // Real keyboard input: type into the editor.
    desk.dispatch(`hl.dsp.focus({ window = "address:${editor.address}" })`);
    await sleep(600);
    s.caption("Type and click as if you were sitting at it");
    // Keys go to the focused window; no click (that would focus whatever is
    // under the pointer). The first keys start the virtual keyboard, so warm
    // it up with harmless Escapes before typing for real.
    await s.page.focus("#screen");
    for (let i = 0; i < 3; i++) { await s.page.keyboard.press("Escape"); await sleep(250); }
    await sleep(500);
    await s.page.keyboard.press("o"); await sleep(500);
    await typeSlowly(s.page, "- [ ] Pasteis de nata at Manteigaria");
    await sleep(300);
    await s.page.keyboard.press("Escape");
    await sleep(1800);
    // The workspace strip: switch workspaces on the real machine.
    s.caption("Switch workspaces from the strip");
    await s.page.click('.ws:nth-child(8)'); await sleep(1600);
    await s.page.click('.ws:nth-child(7)'); await sleep(2000);
    // The Spaces panel: give a window to another machine by dragging it to
    // the top edge (real SUPER-drag on the desktop, seen through the view).
    if (PEER) {
      desk.dispatch(`hl.dsp.focus({ workspace = "7" })`);
      await sleep(600);
      const [w, h] = desk.screen();
      // The browser: its tabs and site data reopen on the other machine.
      const win = desk.clients().find(c => c.address === demoBrowser.address) || demoBrowser;
      const fx = (win.at[0] + win.size[0] / 2) / w, fy = (win.at[1] + win.size[1] / 2) / h;
      s.caption("Drag a window to the top: give it to another machine");
      peerBefore = new Set(peer.clients().map(c => c.address));
      desk.input(`move ${fx} ${fy}`, "sleep 300", "key 125 down", "sleep 120", "button 272 down", "sleep 250",
        `move ${fx} ${fy * 0.7}`, "sleep 180", `move ${fx} ${fy * 0.4}`, "sleep 180", `move ${fx} 0.005`, "sleep 400");
      const sp = await waitFor(() => { const st = desk.spacesState(); return st.opened && st.mode === "drag" && !st.loading && st.machines.length > 1 && st; }, "Spaces open", 10_000).catch(() => null);
      if (sp) {
        const target = sp.machines.indexOf(PEER);
        const [cx, cy] = desk.spaces("cellCenter", `${target} 4`).split(" ").map(Number);
        await sleep(900);
        desk.input(`move ${(cx - 120) / w} ${(cy + 2) / h}`, "sleep 250", `move ${cx / w} ${(cy + 2) / h}`, "sleep 900");
        desk.input("button 272 up", "sleep 60", "key 125 up");
        // Arrived: the peer's own window list shows the new Chromium window.
        arrived = await waitFor(() => peer.clients().find(c => !peerBefore.has(c.address) && /chrom/i.test(c.class)), "browser on the peer", 25_000).catch(() => null);
        s.caption(arrived ? `On ${PEER}: ${arrived.title}` : "Given");
        await sleep(2000);
      } else {
        desk.input("button 272 up", "sleep 60", "key 125 up");
      }
    }
    await s.done();
  }

  // ---- 2. Phone: a screen shaped like the phone -----------------------------
  {
    // iPhone 15 Pro Max as a home-screen app: the full 430x932 screen. The
    // page leaves room for the status bar (59px) and the home indicator
    // (34px) like iOS's safe areas; demo-launch.py draws them.
    const iphone = { ...devices["iPhone 15 Pro Max"], viewport: { width: 430, height: 932 }, screen: { width: 430, height: 932 } };
    const s = await scene("phone", iphone, { w: 1290, h: 2796 });
    const t = touch(s.cdp, s);
    // iOS safe areas (a headless browser reports none). A more specific
    // selector than the page's own :root, added once the page has loaded.
    await s.page.addInitScript(() => document.addEventListener("DOMContentLoaded", () => {
      const st = document.createElement("style");
      st.textContent = "html:root { --safe-t: 59px; --safe-b: 34px; }";
      document.head.appendChild(st);
    }));
    await openView(s.page, { workspace: "5" });
    await waitFor(() => desk.monitors().find(m => m.name.startsWith("OSP-PHONE"))?.activeWorkspace.id === 5, "phone screen", 15_000);
    await s.page.waitForFunction(() => state.windows.filter(w => w.workspace === 5).length >= 2, null, { timeout: 15_000 });
    // A tall phone screen splits side by side, which makes two skinny
    // columns: stack them instead, each the full width of the phone.
    await sleep(800);
    const side = () => { const [a, b] = desk.clients().filter(c => c.workspace.id === 5); return a && b && a.at[1] === b.at[1]; };
    if (side()) { desk.dispatch(`hl.dsp.focus({ window = "address:${pShell.address}" })`); desk.dispatch(`hl.dsp.layout("togglesplit")`); }
    await sleep(1200);
    await s.start();
    s.caption("On your phone, Omarchy re-tiles for a phone-shaped screen");
    await sleep(3500);

    // Long-press a window and drag it onto the other: swap.
    s.caption("Long-press a window and drag it onto another to swap");
    const ca = await centre(s.page, pEditor.address), cb = await centre(s.page, pShell.address);
    await t.down(ca); await sleep(650);
    for (let i = 1; i <= 24; i++) { await t.move({ x: ca.x + (cb.x - ca.x) * i / 24, y: ca.y + (cb.y - ca.y) * i / 24 }); await sleep(16); }
    await sleep(200); await t.up(); await sleep(2400);

    // Move a window to another workspace: long-press, let go, tap 4.
    s.caption("Long-press, let go, move it to another workspace");
    const cm = await centre(s.page, pShell.address);
    await t.down(cm); await sleep(650); await t.up(); await sleep(700);
    await tapOn(s, s.page.locator('#tile-ws button[data-ws="4"]'));
    await sleep(1600);
    // And bring it back, so the rest of the take is unchanged.
    desk.dispatch(`hl.dsp.window.move({ workspace = "5", follow = false, window = "address:${pShell.address}" })`);
    await sleep(900);

    // Type with the phone's keyboard.
    s.caption("Type with your phone's keyboard");
    desk.dispatch(`hl.dsp.focus({ window = "address:${pShell.address}" })`);
    await sleep(500);
    await tapOn(s, s.page.locator("#kbd-btn")); await sleep(500);
    for (const ch of "echo \"sent from my phone\"") {
      await s.page.locator("#kbd").evaluate((el, c) => { el.value += c; el.dispatchEvent(new InputEvent("input", { bubbles: true })); }, ch);
      await sleep(60);
    }
    await s.page.locator("#kbd").press("Enter"); await sleep(1800);
    await tapOn(s, s.page.locator("#kbd-btn")); await sleep(800);

    // Swipe: finger right goes to the previous workspace (5 -> 4), then back.
    s.caption("Swipe between workspaces");
    const mid = { x: 215, y: 520 };
    await t.down(mid);
    for (let i = 1; i <= 10; i++) { await t.move({ x: mid.x + i * 22, y: mid.y }); await sleep(12); }
    await t.up(); await sleep(2200);
    await t.down(mid);
    for (let i = 1; i <= 10; i++) { await t.move({ x: mid.x - i * 22, y: mid.y }); await sleep(12); }
    await t.up(); await sleep(2200);

    // ---- 3. An agent signs in for you, and asks for the 2FA code -------------
    s.caption("Agents get a workspace of their own, and never take yours");
    desk.mcp("claim_space", { agent: AGENT, task: "download last month's bank statement" });
    desk.mcp("open_browser", { agent: AGENT, signed_in: false, urls: [BANK] });
    desk.mcp("set_status", { agent: AGENT, status: "signing in to Harbor Credit Union" });
    const ws = desk.mcp("get_space", { agent: AGENT }).workspace;
    await tapOn(s, s.page.locator(`.ws:nth-child(${ws})`)); await sleep(2200);
    // The agent fills in the login form on camera, a few characters at a time.
    const typeAs = async (sel, text) => {
      desk.mcp("browser_click", { agent: AGENT, selector: sel });
      for (let i = 0; i < text.length; i += 3) { desk.mcp("browser_type", { agent: AGENT, text: text.slice(i, i + 3) }); await sleep(90); }
    };
    await typeAs("#user", "alex.morgan");
    await sleep(500);
    await typeAs("#pass", "correct-horse");
    await sleep(600);
    desk.mcp("browser_click", { agent: AGENT, selector: "button" });
    await sleep(1600);
    desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
    // Where the code field is in the page (read now: reading stays allowed,
    // but taking over pauses the agent's other tools).
    const codeRect = desk.mcp("browser_eval", { agent: AGENT, expression: "(() => { const r = document.getElementById('code').getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; })()" });
    const viewport = desk.mcp("browser_eval", { agent: AGENT, expression: "({ w: innerWidth, h: innerHeight })" });
    s.caption("It stops at the 2FA code, and your phone asks you");
    desk.mcp("ask_for_help", { agent: AGENT, message: "Harbor Credit Union texted you a code. Can you enter it?" });
    await s.page.waitForSelector("#banner.on", { timeout: 15_000 });
    await sleep(500);
    desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
    await sleep(2600);
    s.caption("Take over: the agent pauses, and you type the code");
    await tapOn(s, s.page.locator("#takeover"));
    await s.page.waitForSelector("#control.on", { timeout: 15_000 });
    await sleep(1200);
    // Tap the code field in the agent's browser, then type the code. Where it
    // is on screen: the field's place in the page (read before taking over),
    // offset by the browser window's position and its toolbar.
    const field = await s.page.evaluate(([fr, inner]) => {
      const w = state.windows.find(x => x.class === "omaspace-agent");
      const t = tileRect(w), r = document.getElementById("stage").getBoundingClientRect();
      const k = t.w / w.size[0];                      // stage px per logical px
      const top = w.size[1] - inner.h;                 // browser chrome above the page
      return { x: r.left + t.x + (fr.x + fr.w / 2) * k, y: r.top + t.y + (top + fr.y + fr.h / 2) * k };
    }, [codeRect, viewport]);
    await t.down(field); await sleep(60); await t.up(); await sleep(800);
    await sleep(300);
    await tapOn(s, s.page.locator("#kbd-btn")); await sleep(500);
    for (const ch of "482915") {
      await s.page.locator("#kbd").evaluate((el, c) => { el.value += c; el.dispatchEvent(new InputEvent("input", { bubbles: true })); }, ch);
      await sleep(260);
    }
    await sleep(700);
    await tapOn(s, s.page.locator("#kbd-btn"));
    // The page signs in once all six digits are in.
    // (Reading is allowed while you're in control; acting isn't.)
    await waitFor(() => /Accounts/.test(desk.mcp("browser_read", { agent: AGENT, max_chars: 200 }).title || ""), "signed in", 8000).catch(() => console.log("note: the code didn't sign in"));
    await sleep(2200);
    s.caption("Hand back, and it carries on");
    await tapOn(s, s.page.locator("#handback"));
    desk.mcp("set_status", { agent: AGENT, status: "signed in, downloading the statement" });
    await sleep(3200);
    // Every machine on your tailnet: the switcher lists them, with who's
    // watching and which agents are working on each.
    s.caption("All your Omarchy machines, one tap apart");
    await tapOn(s, s.page.locator("#host"));
    if (PEER) await s.page.waitForSelector(`#machines-list .m[data-name="${PEER}"] .who span`, { timeout: 8000 }).catch(() => {});
    await sleep(3800);
    await tapOn(s, s.page.locator("#host"));
    await sleep(800);
    s.caption("omaspace · github.com/cole-robertson/omaspace");
    await sleep(1500);
    await s.done();
  }
} finally {
  if (peer) {
    if (arrived) {
      fs.writeFileSync(`${OUT}/peer.json`, JSON.stringify({ peer: PEER, title: arrived.title, workspace: arrived.workspace.id }));
      peer.dispatch(`hl.dsp.window.close({ window = "address:${arrived.address}" })`);
    }
    for (const c of peer.windowsOn(4)) if (!peerWsBefore.has(c.address)) peer.dispatch(`hl.dsp.window.close({ window = "address:${c.address}" })`);
    peer.sh(`rm -rf ${shq(DEMO)}`, { check: false });
  }
  try { desk.spaces("dismiss"); } catch {}
  try { desk.mcp("release_space", { agent: AGENT }); } catch {}
  for (const w of opened) desk.dispatch(`hl.dsp.window.close({ window = "address:${w.address}" })`);
  await browser.close();
  site.close();
  desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
  desk.dispatch(`hl.dsp.focus({ workspace = "6" })`);
  fs.rmSync(DEMO, { recursive: true, force: true });
}
console.log("recorded into", OUT);
