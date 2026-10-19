import { withPage, check } from "./lib/cortex-check.mjs";
import { open, startGame, state, head } from "./common.mjs";

const board = (page) => page.evaluate(() => document.getElementById("board").toDataURL());

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await startGame(page);
  await page.waitForTimeout(700);
  const h = head(await state(page));
  check("test mode never moves on its own", h === "10,10", h);

  await open(page, ctx, "");
  await page.locator("#start").click({ timeout: 2000 });
  const before = await board(page);
  await page.waitForTimeout(700);
  check("without ?test=1 the snake moves in real time", (await board(page)) !== before);
});
