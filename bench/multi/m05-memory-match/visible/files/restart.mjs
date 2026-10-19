import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, flip, count, size, faceUp } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, "easy", 8);
  await flip(page, 0);
  await flip(page, 1);
  await flip(page, 2);
  check("cards are face up before restart", (await faceUp(page)) === 3);

  await page.locator("#restart").click({ timeout: 2000 });
  check("restart turns every card face down", (await faceUp(page)) === 0);
  check("restart sets moves to 0", (await text(page, "#moves")) === "0");
  check("restart keeps the size", (await count(page)) === 8 && (await page.inputValue("#size")) === "easy");
});
