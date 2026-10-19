import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, head, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await step(page, TICK);
  let h = head(await state(page));
  check("moves one cell right per 150 ms", h === "11,10", h);
  await page.keyboard.press("ArrowDown");
  await step(page, TICK);
  h = head(await state(page));
  check("down arrow turns down", h === "11,11", h);
  await page.keyboard.press("ArrowLeft");
  await step(page, TICK);
  h = head(await state(page));
  check("left arrow turns left", h === "10,11", h);
});
