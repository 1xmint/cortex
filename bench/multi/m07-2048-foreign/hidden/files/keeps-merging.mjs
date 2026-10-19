import { withPage, check } from "./lib/cortex-check.mjs";
import { load, grid, press, score, best } from "./common.mjs";

const rows = [
  [2, 2, 4, 4],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  await press(page, "ArrowLeft");
  const after = await grid(page);
  check("pairs merge in a slide", after[0][0] === 4 && after[0][1] === 8, JSON.stringify(after[0]));
  check("a new tile appears", after.flat().filter(Boolean).length === 3, JSON.stringify(after));
  check("the merges add to the score", (await score(page)) === 12, String(await score(page)));
  check("the best score follows", (await best(page)) === 12, String(await best(page)));

  await page.reload();
  await page.locator(".tile-container .tile").first().waitFor({ state: "attached" });
  await page.waitForTimeout(300);
  check("the best score survives a reload", (await best(page)) === 12, String(await best(page)));
  check("the game survives a reload", JSON.stringify(await grid(page)) === JSON.stringify(after), JSON.stringify(await grid(page)));
});
