import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);

  // Straight up into the bottom of brick (4, 100).
  await setBall(page, 32, 122, 0, -0.3);
  await step(page, 10);
  const s = await state(page);
  check("the brick breaks", s.bricks.length === 31 && !s.bricks.some((b) => b.x === 4 && b.y === 100), s.bricks.length);
  check("score goes up by 10", s.score === 10, s.score);
  check("score is shown", (await text(page, "#score")) === "10");
  check("ball bounces back down", s.ball.vy > 0, s.ball);
});
