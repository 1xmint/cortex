import { withPage, check } from "./lib/cortex-check.mjs";
import { load, shape, board, press, undo, score, moves } from "./common.mjs";

const rows = [
  [2, 2, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 8],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  await press(page, "ArrowLeft");
  await undo(page);
  await page.reload();
  await page.locator(".tile-container .tile").first().waitFor({ state: "attached" });
  await page.waitForTimeout(300);
  check("the undone board is what was saved", (await board(page)) === shape(rows), await board(page));
  check("the undone score is what was saved", (await score(page)) === 0, String(await score(page)));
  check("the undone move count is what was saved", (await moves(page)) === "0", await moves(page));
});
