import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, food, head, text, TICK } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  for (const x of [11, 12]) {
    await food(page, x, 10);
    await step(page, TICK);
    await food(page, 0, 19);
  }
  let s = await state(page);
  check("snake grew to 5", s.snake.length === 5 && head(s) === "12,10", JSON.stringify(s));
  for (const key of ["ArrowDown", "ArrowLeft"]) {
    await page.keyboard.press(key);
    await step(page, TICK);
  }
  s = await state(page);
  check("still alive before the bite", !s.over && head(s) === "11,11", JSON.stringify(s));
  await page.keyboard.press("ArrowUp");
  await step(page, TICK);
  s = await state(page);
  check("running into its own body ends the game", s.over === true && s.running === false, JSON.stringify(s));
  check("game-over is shown", await page.locator("#game-over").isVisible());
  check("final score stays 2", (await text(page, "#score")) === "2");
});
