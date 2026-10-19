import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, size, win } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, "easy", 8);
  await win(page);
  check("easy best is 4", (await text(page, "#best")) === "4", await text(page, "#best"));

  await open(page, ctx);
  check("medium has no best after reload", (await text(page, "#best")) === "-", await text(page, "#best"));
  await size(page, "easy", 8);
  check("easy best survives a reload", (await text(page, "#best")) === "4", await text(page, "#best"));
});
