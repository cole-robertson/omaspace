import { defineConfig } from "@playwright/test";

// The machines are shared real desktops, so tests run one at a time.
export default defineConfig({
  testDir: "./tests",
  workers: 1,
  fullyParallel: false,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  retries: 0,
  reporter: [["list"], ["html", { open: "never", outputFolder: "report" }]],
  use: {
    browserName: "chromium",
    // A Chromium with the H.264 decoder the live view needs (Playwright's own
    // build lacks it): Omarchy's, or set OMASPACE_E2E_CHROMIUM.
    launchOptions: { executablePath: process.env.OMASPACE_E2E_CHROMIUM || "/usr/bin/chromium", args: ["--ozone-platform=headless"] },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
});
