import { withPage, check } from "./lib/cortex-check.mjs";
import { open, state, cells, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("start screen is shown on load", await page.locator("#start-screen").isVisible());
  const before = await state(page);
  check("no game runs before Start", before.running === false, JSON.stringify(before));
  await page.locator("#start").click();
  check("start screen hides", !(await page.locator("#start-screen").isVisible()));
  const s = await state(page);
  check("game is running", s.running === true && s.over === false, JSON.stringify(s));
  check("snake starts at (10,10) (9,10) (8,10)", cells(s) === "10,10 9,10 8,10", cells(s));
  check("snake starts moving right", s.dir === "right", s.dir);
  check("score shows 0", (await text(page, "#score")) === "0");
});
