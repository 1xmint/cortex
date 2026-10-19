import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  // Park the ball so it cannot be lost while the paddle moves.
  await setBall(page, 240, 200, 0, 0);

  await page.keyboard.down("ArrowRight");
  await step(page, 100);
  let x = (await state(page)).paddle.x;
  check("right arrow moves 0.5 px per ms", Math.abs(x - 250) < 1, x);
  await step(page, 1000);
  await page.keyboard.up("ArrowRight");
  x = (await state(page)).paddle.x;
  check("paddle stops at the right wall", Math.abs(x - 400) < 1, x);

  await page.keyboard.down("ArrowLeft");
  await step(page, 100);
  x = (await state(page)).paddle.x;
  check("left arrow moves 0.5 px per ms", Math.abs(x - 350) < 1, x);
  await step(page, 1000);
  await page.keyboard.up("ArrowLeft");
  x = (await state(page)).paddle.x;
  check("paddle stops at the left wall", Math.abs(x) < 1, x);

  await step(page, 100);
  x = (await state(page)).paddle.x;
  check("paddle stays put once the key is up", Math.abs(x) < 1, x);
});
