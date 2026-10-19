import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await setBall(page, 240, 200, 0, 0);
  const box = await page.locator("#board").boundingBox();

  await page.mouse.move(box.x + 100, box.y + 150);
  let x = (await state(page)).paddle.x;
  check("paddle centre follows the mouse", Math.abs(x - 60) < 2, x);

  await page.mouse.move(box.x + 5, box.y + 150);
  x = (await state(page)).paddle.x;
  check("mouse cannot push the paddle off the left", Math.abs(x) < 1, x);

  await page.mouse.move(box.x + 475, box.y + 150);
  x = (await state(page)).paddle.x;
  check("mouse cannot push the paddle off the right", Math.abs(x - 400) < 1, x);
});
