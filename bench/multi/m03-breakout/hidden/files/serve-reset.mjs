import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await setBall(page, 240, 200, 0, 0);
  await page.keyboard.down("ArrowLeft");
  await step(page, 200);
  await page.keyboard.up("ArrowLeft");

  await setBall(page, 100, 330, 0, 0.3);
  await step(page, 5);
  const s = await state(page);
  check("a life is lost", s.lives === 2, s.lives);
  check("paddle goes back to x 200", Math.abs(s.paddle.x - 200) < 1, s.paddle);
  const b = s.ball;
  const near = (a, e) => Math.abs(a - e) < 2;
  check("ball is served again", near(b.x, 240) && near(b.y, 280) && b.vx === 0.15 && b.vy === -0.3, b);
});
