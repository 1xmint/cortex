import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, clearLevel, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await page.keyboard.down("ArrowRight");
  await page.evaluate(() => window.game.step(20));
  await page.keyboard.up("ArrowRight");
  await clearLevel(page);

  const s = await state(page);
  check("clearing level 1 starts level 2", s.screen === "playing" && s.level === 2, s.screen);
  check("level 2 has 40 bricks", s.bricks.length === 40, s.bricks.length);
  check("level 2 adds a fifth row", s.bricks.some((b) => b.x === 4 && b.y === 120), s.bricks.length);
  check("score carries over", s.score === 320, s.score);
  check("lives carry over", s.lives === 3, s.lives);
  check("paddle goes back to x 200", s.paddle.x === 200, s.paddle);
  // The last step may run on a few milliseconds past the serve.
  const b = s.ball;
  const near = (a, e) => Math.abs(a - e) < 4;
  check("ball is served again", near(b.x, 240) && near(b.y, 280) && b.vx === 0.15 && b.vy === -0.3, b);
  check("level is shown", (await text(page, "#level")) === "2");
  check("not won yet", !(await page.locator("#won").isVisible()));
});
