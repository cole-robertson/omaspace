// A page driving omaspace's live view (the web remote), with helpers that
// act like a person: tap a workspace, tap a window, type on the phone
// keyboard, press key combos, open the Files panel, drop files, read stats.

import { expect } from "@playwright/test";
import { waitFor } from "./machine.js";

export class View {
  constructor(page, machine) {
    this.page = page;
    this.machine = machine;
  }

  /** Wait until frames are decoding onto the canvas. */
  async ready(timeout = 25000) {
    await this.page.waitForFunction(() => document.getElementById("screen")?.width > 300, null, { timeout });
    await this.page.waitForFunction(() => /[1-9]\d* fps/.test(document.getElementById("stats")?.textContent || ""), null, { timeout });
  }

  /** Bitrate, frame rate and stream size as shown in the stats pill. */
  async stats() {
    const t = await this.page.textContent("#stats");
    const m = t.match(/([\d.]+) Mb\/s · (\d+) fps · (\d+)×(\d+)/);
    return m ? { mbps: +m[1], fps: +m[2], width: +m[3], height: +m[4] } : null;
  }

  /** The workspace the strip shows as active. */
  activeWorkspace() { return this.page.evaluate(() => Number(document.querySelector(".ws.on")?.textContent)); }

  async tapWorkspace(n) {
    const loc = this.page.locator(`.ws:nth-child(${n})`);
    (await this.isTouch()) ? await loc.tap() : await loc.click();
  }

  isTouch() { return this.page.evaluate(() => navigator.maxTouchPoints > 0); }

  /** Click or tap at a fraction of the stream (0..1, 0..1). */
  async pointAt(fx, fy) {
    const box = await this.page.locator("#stage").boundingBox();
    const { x, y } = await this.page.evaluate(({ fx, fy }) => {
      const c = document.getElementById("screen"), r = document.getElementById("stage").getBoundingClientRect();
      const m = new DOMMatrix(getComputedStyle(c).transform);
      return { x: m.e + fx * c.width * m.a + r.left, y: m.f + fy * c.height * m.d + r.top };
    }, { fx, fy });
    (await this.isTouch()) ? await this.page.touchscreen.tap(x, y) : await this.page.mouse.click(x, y);
    void box;
  }

  /** Type text the way each device would: phone keyboard or real keys. */
  async type(text) {
    if (await this.isTouch()) {
      await this.page.tap("#kbd-btn");
      const lines = text.split("\n");
      for (let i = 0; i < lines.length; i++) {
        // Append, as a keyboard does (each line starts empty after Enter).
        if (lines[i]) await this.page.locator("#kbd").evaluate((el, t) => { el.value += t; el.dispatchEvent(new InputEvent("input", { bubbles: true })); }, lines[i]);
        if (i < lines.length - 1) await this.page.locator("#kbd").press("Enter");
      }
    } else {
      await this.page.focus("#screen");
      const shifted = { '%': "Digit5", '"': "Quote", ">": "Period", "<": "Comma", "&": "Digit7", ":": "Semicolon", "!": "Digit1", "|": "Backslash", "?": "Slash", "_": "Minus", "+": "Equal", "(": "Digit9", ")": "Digit0", "~": "Backquote", "$": "Digit4", "*": "Digit8" };
      for (const ch of text) {
        if (ch === "\n") await this.page.keyboard.press("Enter");
        else if (shifted[ch]) { await this.page.keyboard.down("Shift"); await this.page.keyboard.press(shifted[ch]); await this.page.keyboard.up("Shift"); }
        else if (ch === " ") await this.page.keyboard.press("Space");
        else if (ch === "'") await this.page.keyboard.press("Quote");
        else await this.page.keyboard.type(ch);
      }
    }
  }

  /** Act like the iPhone keyboard's dictation: it types a draft, then keeps
   *  rewriting it as it hears more (each step replaces the field's text). */
  async dictate(drafts) {
    await this.page.tap("#kbd-btn");
    for (const d of drafts) {
      await this.page.locator("#kbd").evaluate((el, t) => { el.value = t; el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText" })); }, d);
      await this.page.waitForTimeout(120);
    }
  }

  /** A key combo through the sticky modifier bar, e.g. combo(["SUPER"], "Digit1"). */
  async combo(mods, code) {
    for (const m of mods) await this.page.click(`[data-mod="${{ SUPER: 64, SHIFT: 1, CTRL: 4, ALT: 8 }[m]}"]`);
    await this.page.focus("#screen");
    await this.page.keyboard.press(code);
  }

  /** Tap a dock action; on a phone it lives in the Omarchy sheet behind #more-btn. */
  async action(id) {
    if (!(await this.page.locator(id).isVisible())) await this.tapOrClick("#more-btn");
    await this.tapOrClick(id);
  }
  async tapOrClick(sel) { (await this.isTouch()) ? await this.page.tap(sel) : await this.page.click(sel); }

  async openWindows() {
    await this.action("#windows-btn");
    await this.page.waitForSelector("aside.on .win");
  }

  /** Press an action button on a window card, e.g. windowAction("notes", "Float"). */
  async windowAction(titlePart, label) {
    const card = this.page.locator(".win", { hasText: titlePart }).first();
    if (label.startsWith("Move to ")) await card.locator("select").selectOption(label.slice(8));
    else await card.locator("button", { hasText: label }).first().click();
  }

  // ---- files ------------------------------------------------------------------

  async openFiles() {
    await this.action("#files-btn");
    await this.page.waitForSelector("#files.on #files-list .row");
  }

  async fileNames() {
    return this.page.evaluate(() => [...document.querySelectorAll("#files-list .n")].map(a => a.textContent.replace(/^📁 /, "")));
  }

  /** Upload local files through the Upload button; resolves with toast texts. */
  async upload(paths) {
    await this.page.setInputFiles("#file-input", paths);
    return this.lastToasts(paths.length);
  }

  /** Drop in-memory files onto the live view (desktop drag and drop). */
  async drop(files) {
    const dt = await this.page.evaluateHandle(fs => {
      const t = new DataTransfer();
      for (const f of fs) t.items.add(new File([f.content], f.name));
      return t;
    }, files);
    await this.page.dispatchEvent("#stage", "dragenter", { dataTransfer: dt });
    await this.page.dispatchEvent("#stage", "drop", { dataTransfer: dt });
    return this.lastToasts(files.length);
  }

  async lastToasts(n) {
    await this.page.waitForFunction(n => [...document.querySelectorAll(".toast")].filter(t => /^[✓✕]/.test(t.textContent)).length >= n, n, { timeout: 120_000 });
    return this.page.evaluate(() => [...document.querySelectorAll(".toast")].map(t => t.textContent).filter(t => /^[✓✕]/.test(t)));
  }

  /** Download a file from the Files panel; resolves with the saved path. */
  async download(name, saveTo) {
    const link = this.page.locator("#files-list a", { hasText: name });
    const [dl] = await Promise.all([this.page.waitForEvent("download"), (await this.isTouch()) ? link.tap() : link.click()]);
    await dl.saveAs(saveTo);
    return saveTo;
  }

  async screenshot(path) { return this.page.screenshot({ path }); }
  async close() { await this.page.close(); }
}

export { expect, waitFor };
