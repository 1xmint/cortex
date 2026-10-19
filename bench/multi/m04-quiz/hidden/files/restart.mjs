import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text, visible, finish } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  await page.locator("#start").click({ timeout: 2000 });
  await finish(page, qs, (q) => q.answer);

  await page.locator("#restart").click({ timeout: 2000 });
  check("restart button hides the results", !(await visible(page, "#results")));
  check("restart button shows a question", await visible(page, "#question"));
  check("restart button resets the score", (await text(page, "#score")) === "0");
  check("restart button resets the timer", (await text(page, "#timer")) === "10");

  // Score some points, then finish and restart with Enter.
  await finish(page, qs, (q) => q.answer);
  check("results show again", await visible(page, "#results"));
  await page.keyboard.press("Enter");
  check("Enter hides the results", !(await visible(page, "#results")));
  check("Enter shows a question", await visible(page, "#question"));
  check("Enter resets the score", (await text(page, "#score")) === "0");
});
