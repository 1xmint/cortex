import { withPage, check } from "./lib/cortex-check.mjs";
import { open, text, size, win, miss } from "./common.mjs";

const restart = (page) => page.locator("#restart").click({ timeout: 2000 });

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, "easy", 8);
  await win(page);
  check("easy best is 4", (await text(page, "#best")) === "4", await text(page, "#best"));

  await size(page, "medium", 16);
  check("medium has no best yet", (await text(page, "#best")) === "-", await text(page, "#best"));
  await miss(page);
  await win(page);
  check("medium best counts the miss", (await text(page, "#best")) === "9", await text(page, "#best"));

  await size(page, "easy", 8);
  check("easy keeps its own best", (await text(page, "#best")) === "4", await text(page, "#best"));
  await miss(page);
  await win(page);
  check("a worse game keeps the best", (await text(page, "#best")) === "4", await text(page, "#best"));

  await size(page, "medium", 16);
  await restart(page);
  await win(page);
  check("a better game lowers the best", (await text(page, "#best")) === "8", await text(page, "#best"));
});
