import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);

  // Open a gap by breaking brick (64, 100) from below...
  await setBall(page, 92, 122, 0, -0.3);
  await step(page, 10);
  // ...then roll the ball left along that gap into the side of brick (4, 100).
  await setBall(page, 92, 108, -0.3, 0);
  await step(page, 100);
  const s = await state(page);
  check("side hit breaks the brick", !s.bricks.some((b) => b.x === 4 && b.y === 100) && s.score === 20, s.score);
  check("side hit flips the horizontal speed", s.ball.vx > 0, s.ball);
  check("side hit keeps the vertical speed", s.ball.vy === 0, s.ball);
});
