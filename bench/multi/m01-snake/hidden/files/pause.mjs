import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, head, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await page.keyboard.press("Space");
  let s = await state(page);
  check("space pauses", s.paused === true, JSON.stringify(s));
  await step(page, TICK * 10);
  s = await state(page);
  check("a paused snake does not move", head(s) === "10,10", head(s));
  await page.keyboard.press("Space");
  s = await state(page);
  check("space again resumes", s.paused === false, JSON.stringify(s));
  await step(page, TICK);
  s = await state(page);
  check("moves again after resuming", head(s) === "11,10", head(s));
});
