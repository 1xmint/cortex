import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, head } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await step(page, 149);
  let h = head(await state(page));
  check("no move before 150 ms", h === "10,10", h);
  await step(page, 1);
  h = head(await state(page));
  check("moves at exactly 150 ms", h === "11,10", h);
  await page.keyboard.press("ArrowLeft");
  await step(page, 150);
  let s = await state(page);
  check("reversing straight back is ignored", head(s) === "12,10" && s.dir === "right", JSON.stringify(s));
  await page.keyboard.press("ArrowUp");
  await step(page, 150);
  h = head(await state(page));
  check("up arrow turns up", h === "12,9", h);
  await page.keyboard.press("ArrowRight");
  await step(page, 150);
  h = head(await state(page));
  check("right arrow turns right", h === "13,9", h);
});
