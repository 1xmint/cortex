import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state } from "./common.mjs";

const board = (page) => page.evaluate(() => document.getElementById("board").toDataURL());

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await page.waitForTimeout(700);
  const b = (await state(page)).ball;
  check("test mode never moves on its own", b.x === 240 && b.y === 280, b);

  await open(page, ctx, "");
  await startGame(page);
  const before = await board(page);
  await page.waitForTimeout(700);
  check("without ?test=1 the ball moves in real time", (await board(page)) !== before);
});
