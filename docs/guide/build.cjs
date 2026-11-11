// Export the guide to PDF: `node docs/guide/build.cjs [out.pdf]` (needs Playwright's Chromium).
// With `--png DIR` it also writes one PNG per page, for looking at a change without a PDF viewer.
const { chromium } = require("playwright");
const { pathToFileURL } = require("node:url");
const path = require("node:path");

const here = __dirname;
const args = process.argv.slice(2);
const pngAt = args.indexOf("--png");
const pngDir = pngAt >= 0 ? args.splice(pngAt, 2)[1] : null;
const out = args[0] ?? path.join(here, "FreeDAWm-Guide.pdf");

(async () => {
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 816, height: 1056 }, deviceScaleFactor: 2 });
await page.goto(pathToFileURL(path.join(here, "guide.html")).href);
await page.evaluate(() => document.fonts.ready);

// A sheet that has outgrown its page is a layout bug, not something to clip silently.
const overflowing = await page.evaluate(() =>
  [...document.querySelectorAll(".page")]
    .map((p, i) => ({
      n: i + 1,
      // The page itself, or a card squeezed below its content.
      over: Math.max(p.scrollHeight - p.clientHeight, ...[...p.querySelectorAll(".card, .keys-grid")].map((c) => c.scrollHeight - c.clientHeight)),
    }))
    .filter((p) => p.over > 1),
);
if (overflowing.length) {
  console.error("pages overflow:", JSON.stringify(overflowing));
  process.exitCode = 1;
}

await page.pdf({ path: out, width: "8.5in", height: "11in", printBackground: true });
console.log(`wrote ${out}`);

if (pngDir) {
  await page.emulateMedia({ media: "print" });
  const pages = await page.locator(".page").all();
  for (const [i, p] of pages.entries()) {
    await p.screenshot({ path: path.join(pngDir, `page-${String(i + 1).padStart(2, "0")}.png`) });
  }
  console.log(`wrote ${pages.length} PNGs to ${pngDir}`);
}
await browser.close();
})();
