// @ts-check

/** @type {import('@playwright/test').PlaywrightTestConfig} */
const config = {
  testDir: ".",
  timeout: 60_000,
  retries: 0,
  use: {
    browserName: "chromium",
    headless: true,
    viewport: { width: 1100, height: 800 }
  }
};

module.exports = config;

