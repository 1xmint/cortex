import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "   ");
  await add(page, "");
  await page.locator("#new-card").press("Enter");
  check("blank text adds no card", (await page.locator(".card").count()) === 0);
  check("count stays 0", (await count(page, "todo")) === "0");
  await add(page, "  Padded  ");
  const todo = await texts(page, "todo");
  check("text is trimmed", same(todo, ["Padded"]), JSON.stringify(todo));
});
