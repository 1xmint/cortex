import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, food, cells, text, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await food(page, 11, 10);
  await step(page, TICK);
  await food(page, 0, 19);
  await page.keyboard.press("ArrowDown");
  await step(page, TICK * 20);
  check("game is over", (await state(page)).over === true);
  check("restart is visible", await page.locator("#restart").isVisible());
  await page.locator("#restart").click({ timeout: 2000 });
  const s = await state(page);
  check("a new game is running", s.running && !s.over && !s.paused, JSON.stringify(s));
  check("snake is back at the start", cells(s) === "10,10 9,10 8,10" && s.dir === "right", cells(s));
  check("score is back to 0", s.score === 0 && (await text(page, "#score")) === "0", String(s.score));
  check("game-over hides", !(await page.locator("#game-over").isVisible()));
});
