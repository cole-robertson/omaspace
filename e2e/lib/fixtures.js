// Playwright Test fixtures for omaspace: real Omarchy machines and browsers
// acting as a phone and a laptop viewing them.
//
//   import { test, expect } from "../lib/fixtures.js";
//   test("…", async ({ desk, peer, phone, laptop }) => { … });
//
// `desk` is the machine under test (its live view, agents, Spaces panel);
// `peer` is a second machine for transfers between the two. Each is a tailnet
// hostname, reached over SSH unless it is this machine:
//
//   OMASPACE_E2E_DESK  required for every test
//   OMASPACE_E2E_PEER  optional; tests that need two machines skip without it
//
// Every window, file, virtual screen and sync a test creates is cleaned up
// after it, pass or fail.

import { test as base, expect, devices, chromium } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { Machine, waitFor } from "./machine.js";
import { View } from "./view.js";

const DESK = process.env.OMASPACE_E2E_DESK;
const PEER = process.env.OMASPACE_E2E_PEER;

export const test = base.extend({
  desk: async ({}, use) => {
    if (!DESK) throw new Error("set OMASPACE_E2E_DESK to the tailnet hostname of the Omarchy machine to test (see e2e/README.md)");
    const m = new Machine(DESK);
    await use(m);
    await m.cleanup();
  },
  peer: async ({}, use, info) => {
    info.skip(!PEER, "needs a second machine: set OMASPACE_E2E_PEER");
    const m = new Machine(PEER);
    await use(m);
    await m.cleanup();
  },
  /** A phone-sized browser; `phone.open(machine, opts)` returns a View. */
  phone: async ({ browser }, use) => {
    const ctx = await browser.newContext({ ...devices["iPhone 15 Pro Max"], acceptDownloads: true });
    await use(new Viewer(ctx, "Phone"));
    await ctx.close();
  },
  /** A laptop whose microphone plays fixtures/speech.wav (espeak-ng saying "echo spoken words
   *  here"): Chromium's fake capture device, so hold-to-talk records real audio. */
  talker: async ({}, use) => {
    const wav = fileURLToPath(new URL("../fixtures/speech.wav", import.meta.url));
    const browser = await chromium.launch({
      executablePath: process.env.OMASPACE_E2E_CHROMIUM || "/usr/bin/chromium",
      args: ["--ozone-platform=headless", "--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", `--use-file-for-fake-audio-capture=${wav}`],
    });
    const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 }, permissions: ["microphone"] });
    await use(new Viewer(ctx, "Talker"));
    await browser.close();
  },
  /** A laptop-sized browser. */
  laptop: async ({ browser }, use) => {
    const ctx = await browser.newContext({ viewport: { width: 1400, height: 860 }, acceptDownloads: true });
    await use(new Viewer(ctx, "Laptop"));
    await ctx.close();
  },
});

class Viewer {
  constructor(ctx, name) {
    this.ctx = ctx;
    this.name = name;
  }

  /** Open machine's live view; resolves when video is playing. */
  async open(machine, params = {}) {
    const page = await this.ctx.newPage();
    const q = new URLSearchParams({ name: this.name, ...params });
    await page.goto(`${machine.viewUrl()}/?${q}`);
    const view = new View(page, machine);
    await view.ready();
    return view;
  }
}

export { expect, waitFor };
