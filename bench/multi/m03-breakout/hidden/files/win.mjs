import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, clearLevel } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await clearLevel(page);
  await clearLevel(page);

  const s = await state(page);
  check("clearing level 2 wins", s.screen === "won" && s.level === 2, s.screen);
  check("score counts all 72 bricks", s.score === 720, s.score);
  check("win screen shows", await page.locator("#won").isVisible());
  check("lost screen stays hidden", !(await page.locator("#lost").isVisible()));
});
