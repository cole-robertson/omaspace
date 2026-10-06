// How the live view looks on a phone and a laptop: Omarchy theme, one-row
// workspace strip, a dock that fits, glyphs from Omarchy's own font.
// Screenshots land in the test output folder (and the HTML report).

import { test, expect } from "../lib/fixtures.js";

test("phone: one-row strip, dock fits, icons render, Omarchy theme", async ({ desk, phone }, info) => {
  const view = await phone.open(desk, {});
  const page = view.page;
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: info.outputPath("phone.png") });

  const strip = await page.evaluate(() => {
    const ws = [...document.querySelectorAll(".ws")].map(b => b.getBoundingClientRect().top);
    return { rows: new Set(ws.map(Math.round)).size, count: ws.length };
  });
  expect(strip.count).toBe(9);
  expect(strip.rows, "workspace strip never wraps").toBe(1);

  // Every dock button is at least a 40px tap target and the keys are all on screen.
  const dock = await page.evaluate(() => [...document.querySelectorAll(".keys button")].filter(b => b.offsetParent).map(b => { const r = b.getBoundingClientRect(); return { w: r.width, h: r.height, right: r.right }; }));
  for (const b of dock) { expect(b.h).toBeGreaterThanOrEqual(40); expect(b.right).toBeLessThanOrEqual(page.viewportSize().width); }

  // The Omarchy actions are one tap away in a sheet, every one fully on screen.
  await page.tap("#more-btn");
  const sheet = await page.evaluate(() => [...document.querySelectorAll("#acts.on button")].filter(b => b.offsetParent).map(b => { const r = b.getBoundingClientRect(); return { l: r.left, r: r.right, b: r.bottom, h: r.height }; }));
  expect(sheet.length).toBeGreaterThanOrEqual(6);
  for (const b of sheet) { expect(b.l).toBeGreaterThanOrEqual(0); expect(b.r).toBeLessThanOrEqual(page.viewportSize().width); expect(b.h).toBeGreaterThanOrEqual(44); }
  await page.screenshot({ path: info.outputPath("phone-actions.png") });
  await page.tap("#more-btn");

  // Nerd Font icons come from the host's font, so they draw on any phone.
  expect(await page.evaluate(() => document.fonts.check('16px "OmarchyMono"', "󰣇"))).toBe(true);
  // The accent is the host's Omarchy theme accent.
  const accent = desk.sh("grep '^accent' ~/.local/state/omarchy/current/theme/colors.toml | cut -d'\"' -f2");
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim())).toBe(accent);
});

test("phone: Windows opens as a bottom sheet", async ({ desk, phone }, info) => {
  await desk.terminal("$HOME", 7);
  const view = await phone.open(desk, { workspace: "7" });
  await view.action("#windows-btn");
  const sheet = await view.page.locator("aside.on").boundingBox();
  const vp = view.page.viewportSize();
  expect(Math.round(sheet.x)).toBe(0);
  expect(Math.round(sheet.width)).toBe(vp.width);
  await view.page.screenshot({ path: info.outputPath("phone-windows.png") });
});

test("laptop: labelled dock and stats", async ({ desk, laptop }, info) => {
  const view = await laptop.open(desk);
  await view.page.evaluate(() => document.fonts.ready);
  await expect(view.page.locator("#stats")).toBeVisible();
  await expect(view.page.locator("#files-btn")).toContainText("Files");
  await view.page.screenshot({ path: info.outputPath("laptop.png") });
});
