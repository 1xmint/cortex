import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, head, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await page.keyboard.press("ArrowUp");
  await step(page, TICK * 10);
  let s = await state(page);
  check("alive on the top row (10,0)", !s.over && head(s) === "10,0", JSON.stringify(s));
  await step(page, TICK);
  s = await state(page);
  check("leaving the top ends the game", s.over === true, JSON.stringify(s));
});
