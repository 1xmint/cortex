import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, head, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await step(page, TICK * 9);
  let s = await state(page);
  check("alive on the last cell (19,10)", !s.over && head(s) === "19,10", JSON.stringify(s));
  check("game-over is hidden while playing", !(await page.locator("#game-over").isVisible()));
  await step(page, TICK);
  s = await state(page);
  check("leaving the board ends the game", s.over === true && s.running === false, JSON.stringify(s));
  check("game-over is shown", await page.locator("#game-over").isVisible());
});
