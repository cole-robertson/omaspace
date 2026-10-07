// README screenshots (docs/screenshots/): the live view on a laptop and on a
// phone, an agent asking for help, and taking over. Stage a tidy workspace
// first. From e2e/: OMASPACE_E2E_DESK=host OUT=../docs/screenshots node screenshots.mjs
import { chromium, devices } from "@playwright/test";
import { Machine } from "./lib/machine.js";

const desk = new Machine(process.env.OMASPACE_E2E_DESK);
const out = process.env.OUT || "/tmp/osp-shots";
const WS = Number(process.env.WS || 8);
const browser = await chromium.launch({ executablePath: process.env.OMASPACE_E2E_CHROMIUM || "/usr/bin/chromium", args: ["--ozone-platform=headless"] });

async function open(ctx, params) {
  const page = await ctx.newPage();
  await page.goto(`${desk.viewUrl()}/?${new URLSearchParams({ name: "You", ...params })}`);
  await page.waitForFunction(() => /[1-9]\d* fps/.test(document.getElementById("stats")?.textContent || ""), null, { timeout: 30_000 });
  await page.waitForTimeout(2500);
  return page;
}

// Laptop: the whole desktop.
const laptop = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 });
const lp = await open(laptop, {});
await lp.screenshot({ path: `${out}/laptop.png` });
await laptop.close();

// Phone: a phone-shaped screen of the same workspace.
const phone = await browser.newContext({ ...devices["iPhone 15 Pro Max"] });
const pp = await open(phone, { workspace: String(WS) });
await pp.screenshot({ path: `${out}/phone.png` });

// An agent asks for help: the banner, then take over.
desk.mcp("claim_space", { agent: "Claude", task: "book a table for Friday" });
desk.mcp("set_status", { agent: "Claude", status: "comparing three restaurants" });
desk.mcp("ask_for_help", { agent: "Claude", message: "Which time works, 7:00 or 7:30?" });
await pp.waitForSelector("#banner.on", { timeout: 15_000 });
// The desktop's own notification for the request would sit in the video.
desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
await pp.waitForTimeout(1500);
await pp.screenshot({ path: `${out}/phone-help.png` });
await pp.locator("#takeover").tap();
await pp.waitForSelector("#control.on", { timeout: 15_000 });
await pp.waitForTimeout(1500);
await pp.screenshot({ path: `${out}/phone-control.png` });
await pp.locator("#handback").tap();
await pp.waitForTimeout(500);
desk.mcp("release_space", { agent: "Claude" });
await phone.close();
await browser.close();
desk.sh("omarchy-shell -q notifications dismissAll", { check: false });
console.log("shots in", out);
