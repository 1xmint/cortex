import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);

  await setBall(page, 20, 200, -0.3, 0);
  await step(page, 60);
  let b = (await state(page)).ball;
  check("bounces off the left wall", b.vx > 0, b);

  await setBall(page, 460, 200, 0.3, 0);
  await step(page, 60);
  b = (await state(page)).ball;
  check("bounces off the right wall", b.vx < 0, b);

  await setBall(page, 240, 20, 0, -0.3);
  await step(page, 60);
  b = (await state(page)).ball;
  check("bounces off the top wall", b.vy > 0, b);

  await setBall(page, 240, 280, 0, 0.3);
  await step(page, 60);
  b = (await state(page)).ball;
  check("bounces off the paddle", b.vy < 0 && b.y < 300, b);
});
