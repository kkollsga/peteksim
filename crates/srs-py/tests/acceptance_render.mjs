/*
 * R6 acceptance — the Playwright browser-render leg.
 *
 * Loads a self-contained `save_view` HTML export of the full-chain model in
 * headless Chromium, cycles every viewer tab (volume / map / section / wells /
 * charts), watches the console for errors, and asserts the volume actually
 * rendered (its tri-count badge appears). The producer→viewer round-trip the
 * per-repo unit tests cannot cover (doctrine R6): a payload that a per-repo test
 * pronounces "valid" but the real three.js viewer chokes on is caught here.
 *
 * This mirrors petekTools' `viewer_perf/render_bench.mjs` liveness path but is
 * self-contained (no perf caps) so the acceptance suite owns its own render gate.
 *
 * Run:  node acceptance_render.mjs <view.html>
 *   exit 0 = every tab rendered clean, no console errors, volume tri-badge shown
 *   exit 2 = usage; 6 = console errors; 7 = volume never rendered
 *
 * `require('playwright')` honours NODE_PATH, so the Python driver points it at a
 * playwright install anywhere; the driver skips this leg when none resolves.
 */
import { pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import process from "node:process";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const file = process.argv.slice(2).find((a) => !a.startsWith("--"));
if (!file) {
  console.error("usage: node acceptance_render.mjs <view.html>");
  process.exit(2);
}

const browser = await chromium.launch({ args: ["--js-flags=--expose-gc"] });
const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });

const consoleErrors = [];
page.on("console", (m) => {
  if (m.type() === "error") consoleErrors.push(m.text());
});
page.on("pageerror", (e) => consoleErrors.push(String(e)));

await page.goto(pathToFileURL(file).href);

const result = await page.evaluate(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const clickTab = (name) => {
    const t = document.querySelector(`.tab[data-tab="${name}"]`);
    if (t) t.click();
    return !!t;
  };
  const badge = () => document.querySelector("#volume-host div");

  // 1) Volume decode + first render — wait for the tri-count badge.
  const tabs = [];
  clickTab("volume");
  let waited = 0;
  while (waited < 30000) {
    const b = badge();
    if (b && /tris/.test(b.textContent || "")) break;
    await sleep(16);
    waited += 16;
  }
  const volBadge = (badge() && badge().textContent) || null;

  // 2) Every tab renders without throwing (liveness).
  for (const name of ["map", "section", "wells", "charts", "volume"]) {
    tabs.push([name, clickTab(name)]);
    await sleep(60);
  }
  if (window.gc) window.gc();
  return { volBadge, tabs };
}, {});

result.consoleErrors = consoleErrors;
await browser.close();

const fail = (code, msg) => {
  console.log(JSON.stringify({ ...result, failure: msg }));
  process.exit(code);
};
if (consoleErrors.length) fail(6, "console errors: " + consoleErrors.slice(0, 3).join(" | "));
if (!/tris/.test(result.volBadge || "")) fail(7, "volume never rendered (no tri badge)");

console.log(JSON.stringify(result));
