import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, size, win, visible } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("no best before any win", (await text(page, "#best")) === "-");

  await size(page, "easy", 8);
  await win(page);
  check("the game is won", await visible(page, "#won"));
  check("the best shows the winning moves", (await text(page, "#best")) === "4", await text(page, "#best"));
});
