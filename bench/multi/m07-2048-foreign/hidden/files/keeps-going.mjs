import { withPage, check } from "./lib/cortex-check.mjs";
import { load, board, press } from "./common.mjs";

const rows = [
  [1024, 1024, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  await press(page, "ArrowLeft");
  check("2048 wins", await page.locator(".game-message.game-won").isVisible());
  const won = await board(page);
  await press(page, "ArrowRight");
  check("the game waits while the win shows", (await board(page)) === won);

  await page.locator(".keep-playing-button").click({ timeout: 2000 });
  await press(page, "ArrowRight");
  check("Keep going lets play carry on", (await board(page)) !== won, await board(page));
});
