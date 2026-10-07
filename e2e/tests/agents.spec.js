// Agent spaces: an agent gets its own Omarchy workspace on the user's
// machine, a browser signed in as the user, and never takes their focus,
// pointer or workspace. The user watches it from the phone, gets its help
// requests, and can hand a browser over or take it back.

import { test, expect, waitFor } from "../lib/fixtures.js";

const ME = 6; // the user's workspace during these tests
const AGENT = "E2E Agent";

test.beforeEach(async ({ desk }) => {
  try { desk.mcp("release_space", { agent: AGENT }); } catch { }
  desk.dispatch(`hl.dsp.focus({ workspace = "${ME}" })`);
  desk.onCleanup(() => { try { desk.mcp("release_space", { agent: AGENT }); } catch { } });
});

const where = m => ({ ws: m.activeWorkspace(), cursor: m.json("hyprctl -j cursorpos"), focus: m.json("hyprctl -j activewindow")?.address ?? null });

test("an agent works in its own signed-in browser without touching the user's focus, pointer or workspace", async ({ desk }) => {
  const space = desk.mcp("claim_space", { agent: AGENT, task: "read a page" });
  expect(space.workspace).not.toBe(ME);
  const before = where(desk);
  desk.mcp("open_browser", { agent: AGENT, urls: ["data:text/html,<input id=q><button id=go onclick=\"document.title=q.value\">go</button>"] });
  // It types and clicks in its browser.
  desk.mcp("browser_type", { agent: AGENT, selector: "#q", text: "done by the agent" });
  desk.mcp("browser_click", { agent: AGENT, selector: "#go" });
  expect(desk.mcp("browser_read", { agent: AGENT }).title).toBe("done by the agent");
  // Its window is on its workspace; the user's world didn't move.
  const win = desk.clients().find(c => c.class === "omaspace-agent" && c.workspace.id === space.workspace);
  expect(win, "its browser is on its workspace").toBeTruthy();
  expect(where(desk)).toEqual(before);
});

test("the agent's browser is signed in as the user (their cookies came along)", async ({ desk }) => {
  // The user "signs in": a cookie set in their own browser. The agent's copy
  // of the profile must carry it.
  const marker = `omaspace_e2e_signin_${Date.now()}`;
  const mine = await desk.browser(["https://example.org/"], ME);
  desk.onCleanup(() => desk.dispatch(`hl.dsp.window.close({ window = "address:${mine.address}" })`));
  desk.mcp("claim_space", { agent: AGENT, task: "set a cookie as the user" });
  desk.mcp("open_browser", { agent: AGENT, signed_in: false, urls: ["https://example.org/"] });
  desk.mcp("browser_navigate", { agent: AGENT, url: "https://example.org/" }); // waits for the page
  desk.mcp("browser_eval", { agent: AGENT, expression: `document.cookie = "${marker}=yes; path=/; max-age=3600"` });
  desk.mcp("take_browser_back", { agent: AGENT });
  const inUserProfile = () => desk.sh(`cp ~/.config/chromium/Default/Cookies /tmp/osp-ck.db && sqlite3 /tmp/osp-ck.db "select count(*) from cookies where name='${marker}'"; rm -f /tmp/osp-ck.db`);
  await waitFor(() => inUserProfile() === "1", "the user's browser has the sign-in cookie", 20_000);
  // A fresh agent browser seeded from the user's profile has it too.
  desk.mcp("open_browser", { agent: AGENT, urls: ["https://example.org/"] });
  desk.mcp("browser_navigate", { agent: AGENT, url: "https://example.org/" });
  expect(desk.mcp("browser_cookies", { agent: AGENT })).toContain(marker);
});

test("phone: the agent asks for help, you take over (it's paused), then hand it back", async ({ desk, phone }) => {
  desk.mcp("claim_space", { agent: AGENT, task: "sign in to the bank" });
  desk.mcp("open_browser", { agent: AGENT, signed_in: false, urls: ["https://example.org/"] });
  desk.onCleanup(() => desk.sh("omarchy-shell -q notifications dismissAll", { check: false }));
  const view = await phone.open(desk, { workspace: String(ME) });
  desk.onCleanup(() => view.page.evaluate(() => send({ type: "hand_back" })).catch(() => {}));
  desk.mcp("ask_for_help", { agent: AGENT, message: "enter the 2FA code" });
  const banner = view.page.locator("#banner.on");
  await expect(banner).toContainText("enter the 2FA code");
  await view.page.locator("#takeover").tap();
  // You're in control: the bar says so and the agent's actions are refused.
  await expect(view.page.locator("#control")).toContainText("You're in control");
  expect(() => desk.mcp("browser_navigate", { agent: AGENT, url: "https://example.com/" })).toThrow(/taken over/);
  expect(desk.mcp("get_space", { agent: AGENT }).taken_over_by).toBe("Phone");
  // Reading still works, so the agent can wait and watch.
  expect(desk.mcp("browser_tabs", { agent: AGENT }).length).toBeGreaterThan(0);
  // Hand back from the phone: the agent carries on.
  await view.page.locator("#handback").tap();
  await expect(view.page.locator("#control")).toBeHidden();
  await waitFor(() => desk.mcp("get_space", { agent: AGENT }).taken_over_by === null, "agent resumed");
  desk.mcp("browser_navigate", { agent: AGENT, url: "https://example.org/" });
});

test("closing the phone that took over resumes the agent", async ({ desk, phone }) => {
  desk.mcp("claim_space", { agent: AGENT, task: "wait for you" });
  const view = await phone.open(desk, { workspace: String(ME) });
  await view.page.evaluate(() => send({ type: "take_over" }));
  await expect(view.page.locator("#control")).toBeVisible();
  expect(desk.mcp("get_space", { agent: AGENT }).taken_over_by).toBe("Phone");
  await view.page.close();
  await waitFor(() => desk.mcp("get_space", { agent: AGENT }).taken_over_by === null, "agent resumed when the phone left");
});

test("the phone sees the agent's workspace, its status, and its call for help", async ({ desk, phone }) => {
  const space = desk.mcp("claim_space", { agent: AGENT, task: "book a table" });
  desk.mcp("set_status", { agent: AGENT, status: "comparing three places" });
  const view = await phone.open(desk, { workspace: String(ME) });
  const tab = view.page.locator(`.ws[data-ws="${space.workspace}"]`);
  await expect(tab).toHaveClass(/agent/);
  await expect(tab.locator(".who")).toHaveText("E");
  // Go watch it: the bar says who and what.
  await tab.tap();
  await expect(view.page.locator("#agent-bar.on")).toContainText("comparing three places");
  // It asks for help: banner on the phone, the strip turns red.
  desk.mcp("ask_for_help", { agent: AGENT, message: "which time works?" });
  desk.onCleanup(() => desk.sh("omarchy-shell -q notifications dismissAll", { check: false }));
  await expect(view.page.locator("#banner.on")).toContainText("which time works?");
  await expect(tab).toHaveClass(/needs/);
  // Done helping: the agent sees help cleared.
  await view.page.locator("#agent-bar").tap();
  await view.page.locator("#agent-sheet .done").tap();
  await waitFor(() => desk.mcp("get_space", { agent: AGENT }).help === null, "help cleared for the agent");
});

test("hand a browser window to the agent, then take it back", async ({ desk, phone }) => {
  const space = desk.mcp("claim_space", { agent: AGENT, task: "finish the order" });
  const mine = await desk.browser(["https://example.org/", "https://www.iana.org/help/example-domains"], ME);
  // Let Chromium write its session file so the tabs can be read.
  await new Promise(r => setTimeout(r, 4000));
  const view = await phone.open(desk, { workspace: String(ME) });
  await view.page.waitForFunction(a => state.windows.some(w => w.address === a), mine.address, { timeout: 10_000 });
  // Long-press the browser, pick "Hand to E2E Agent".
  const at = await view.page.evaluate(a => { const w = state.windows.find(x => x.address === a), r = tileRect(w), s = document.getElementById("stage").getBoundingClientRect(); return { x: s.left + r.x + r.w / 2, y: s.top + r.y + r.h / 2 }; }, mine.address);
  const cdp = await view.page.context().newCDPSession(view.page);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  await view.page.waitForTimeout(600);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await view.page.locator(`#tile-hand button[data-agent="${AGENT}"]`).tap();
  // The agent's browser has those tabs, on its workspace; the user's window is gone.
  await waitFor(() => !desk.clients().some(c => c.address === mine.address), "user's window closed");
  const tabs = await waitFor(() => { try { const t = desk.mcp("browser_tabs", { agent: AGENT }); return t.length >= 2 && t; } catch { return false; } }, "agent has the tabs", 30_000);
  expect(tabs.map(t => t.url)).toEqual(expect.arrayContaining(["https://example.org/", "https://www.iana.org/help/example-domains"]));
  expect(desk.clients().some(c => c.class === "omaspace-agent" && c.workspace.id === space.workspace)).toBe(true);

  // While it has the browser the agent "signs in" somewhere: a cookie that
  // must come back to the user's own profile with the browser.
  const marker = `omaspace_e2e_${Date.now()}`;
  desk.mcp("browser_navigate", { agent: AGENT, url: "https://example.org/" });
  desk.mcp("browser_eval", { agent: AGENT, expression: `document.cookie = "${marker}=yes; path=/; max-age=3600"` });
  await new Promise(r => setTimeout(r, 1500)); // Chromium flushes cookies to disk

  // Take it back from the agent's sheet: tabs reopen on the user's workspace.
  const before = new Set(desk.clients().map(c => c.address));
  await view.page.evaluate(n => openAgentSheet(n), AGENT);
  await view.page.locator("#agent-sheet .browsers button").first().tap();
  // The user's Chromium restarts (to take the agent's sign-ins) and its tabs
  // reopen on the user's workspace.
  const userTabs = () => JSON.parse(desk.sh(`omaspace snapshot --workspace ${ME} 2>/dev/null`)).workspaces.flatMap(w => w.windows).filter(w => w.kind === "browser").flatMap(w => w.urls);
  await waitFor(() => { const t = userTabs(); return t.includes("https://www.iana.org/help/example-domains") && t; }, "the tabs are back in the user's browser", 30_000);
  // Close every Chromium window this test left (the returned one may be
  // anywhere; the user's browser was restarted), and wait until they're gone,
  // so the next test's session file doesn't carry these tabs.
  // Close the user's Chromium windows this test leaves behind (the returned
  // one, wherever it landed: the restarted browser may reuse addresses), and
  // wait until Chromium has saved a session without them.
  desk.onCleanup(async () => {
    const left = () => desk.clients().filter(c => c.class === "chromium");
    for (const c of left()) desk.dispatch(`hl.dsp.window.close({ window = "address:${c.address}" })`);
    await waitFor(() => left().length === 0, "test browser windows closed", 10_000).catch(() => {});
  });
  await waitFor(() => !desk.clients().some(c => c.class === "omaspace-agent" && c.workspace.id === space.workspace), "agent's browser closed");
  await waitFor(() => desk.mcp("get_space", { agent: AGENT }).browsers.length === 0, "the agent no longer has the browser");
  // The agent's new sign-in is in the user's profile now.
  const has = () => desk.sh(`cp ~/.config/chromium/Default/Cookies /tmp/osp-ck.db && sqlite3 /tmp/osp-ck.db "select count(*) from cookies where name='${marker}'"; rm -f /tmp/osp-ck.db`);
  await waitFor(() => has() === "1", "the cookie the agent made is in the user's Chromium");
});

test("releasing the space closes its browsers and deletes their copy of the user's sign-ins", async ({ desk }) => {
  const space = desk.mcp("claim_space", { agent: AGENT });
  const b = desk.mcp("open_browser", { agent: AGENT });
  expect(desk.exists(`${b.data_dir}/Default/Cookies`)).toBe(true);
  desk.mcp("release_space", { agent: AGENT });
  await waitFor(() => !desk.clients().some(c => c.class === "omaspace-agent" && c.workspace.id === space.workspace), "agent browser closed");
  expect(desk.exists(b.data_dir)).toBe(false);
  expect(desk.mcp("list_spaces", {}).some(s => s.agent === AGENT)).toBe(false);
});

test("an agent's terminal: runs commands and reads the screen while the user keeps their focus", async ({ desk }) => {
  const dir = desk.scratch("agent-term");
  const space = desk.mcp("claim_space", { agent: AGENT, task: "build something" });
  const before = where(desk);
  desk.mcp("open_terminal", { agent: AGENT, dir });
  await waitFor(() => desk.clients().find(c => c.class === "omaspace-agent-term" && c.workspace.id === space.workspace), "agent terminal opened on its workspace");
  const r = desk.mcp("terminal_run", { agent: AGENT, command: "pwd && echo hello-from-the-agent > made.txt && cat made.txt" });
  expect(r.exit_code).toBe(0);
  expect(r.output).toContain(dir);
  expect(r.output).toContain("hello-from-the-agent");
  expect(desk.read(`${dir}/made.txt`)).toBe("hello-from-the-agent");
  // A failing command reports its exit code.
  expect(desk.mcp("terminal_run", { agent: AGENT, command: "false" }).exit_code).toBe(1);
  // Interactive: start a program, type into it, read it, quit with keys.
  desk.mcp("terminal_type", { agent: AGENT, text: "cat\n" });
  desk.mcp("terminal_type", { agent: AGENT, text: "typed into cat\n" });
  await waitFor(() => (desk.mcp("terminal_read", { agent: AGENT }).screen.match(/typed into cat/g) || []).length >= 2, "cat echoed the line");
  desk.mcp("terminal_key", { agent: AGENT, keys: ["C-c"] });
  expect(where(desk)).toEqual(before);
  // Releasing closes it.
  desk.mcp("release_space", { agent: AGENT });
  await waitFor(() => !desk.clients().some(c => c.class === "omaspace-agent-term" && c.workspace.id === space.workspace), "terminal closed on release");
});

test("an agent uses a desktop app (GTK file manager) by its accessibility tree, in the background", async ({ desk }) => {
  const dir = desk.scratch("agent-app");
  desk.write(`${dir}/report-q3.txt`, "x");
  const space = desk.mcp("claim_space", { agent: AGENT, task: "look at files" });
  const before = where(desk);
  const win = desk.mcp("open_app", { agent: AGENT, command: `nautilus --new-window ${dir}` });
  expect(win.workspace).toBe(space.workspace);
  await waitFor(() => desk.mcp("app_read", { agent: AGENT, window: win.address }).elements.some(e => /report-q3\.txt/.test(e.label || "")), "the file shows in its tree", 20_000);
  // Tokens belong to the latest read: read once more, then act on it.
  const tree = desk.mcp("app_read", { agent: AGENT, window: win.address });
  // Click a real button by token: Nautilus's search toggle, then its tree changes.
  const search = tree.elements.find(e => e.role === "toggle button" && /Search Current Folder/.test(e.label || ""));
  expect(search, "a search button in the tree").toBeTruthy();
  expect(desk.mcp("app_click", { agent: AGENT, window: win.address, token: search.token }).route).toBe("accessibility");
  await waitFor(() => desk.mcp("app_read", { agent: AGENT, window: win.address }).elements.some(e => /Filter Search Results/.test(e.label || "")), "search opened");
  expect(where(desk)).toEqual(before);
});

test("an agent can't touch the user's windows", async ({ desk }) => {
  desk.mcp("claim_space", { agent: AGENT });
  const mine = await desk.terminal("$HOME", ME);
  expect(() => desk.mcp("app_read", { agent: AGENT, window: mine.address })).toThrow(/isn't on your workspace/);
  expect(() => desk.mcp("app_click", { agent: AGENT, window: mine.address, x: 10, y: 10 })).toThrow(/isn't on your workspace/);
  expect(() => desk.mcp("close_window", { agent: AGENT, window: mine.address })).toThrow(/isn't on your workspace/);
  expect(desk.clients().some(c => c.address === mine.address)).toBe(true);
});

test("an agent types into an allowlisted app (foot) through the Hyprland plugin, in the background", async ({ desk }) => {
  // The plugin's agent seat only exists after a clean Hyprland start with it
  // loaded (a reload disables it until the desktop restarts).
  test.skip(!/capabilities: .*background mutation enabled|transport: running/.test(desk.sh("hyprctl cua:status 2>&1 || true", { check: false })), "cua plugin input is off until Hyprland restarts");
  const dir = desk.scratch("agent-seat");
  // foot is in qualified-apps (setup adds it); the driver types through cua's
  // plugin on its own input seat.
  desk.mcp("claim_space", { agent: AGENT });
  const before = where(desk);
  const win = desk.mcp("open_app", { agent: AGENT, command: `foot --working-directory=${dir}` });
  await new Promise(r => setTimeout(r, 800));
  const r = desk.mcp("app_type", { agent: AGENT, window: win.address, text: "echo typed-on-the-agent-seat > seat.txt\n" });
  expect(r.route).toBeTruthy();
  await waitFor(() => desk.exists(`${dir}/seat.txt`), "the typed command ran");
  expect(desk.read(`${dir}/seat.txt`)).toBe("typed-on-the-agent-seat");
  expect(where(desk)).toEqual(before);
});
