import { withPage, check } from "./lib/cortex-check.mjs";
import { open, count, size, columns } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("medium is the default", (await page.inputValue("#size")) === "medium");
  check("medium has 16 cards", (await count(page)) === 16);
  check("medium has 4 columns", (await columns(page)) === 4, String(await columns(page)));

  await size(page, "easy", 8);
  check("easy has 8 cards", (await count(page)) === 8);
  check("easy has 4 columns", (await columns(page)) === 4, String(await columns(page)));

  await size(page, "hard", 36);
  check("hard has 36 cards", (await count(page)) === 36);
  check("hard has 6 columns", (await columns(page)) === 6, String(await columns(page)));
});
