import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, step, setBall } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);

  // Comes down near the paddle's left end, drifting right.
  await setBall(page, 210, 280, 0.1, 0.3);
  await step(page, 60);
  const b = (await state(page)).ball;
  check("bounces up off the paddle", b.vy < 0, b);
  check("speed is unchanged", Math.abs(b.vy + 0.3) < 1e-9 && Math.abs(b.vx - 0.1) < 1e-9, b);
});
