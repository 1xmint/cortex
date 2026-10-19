import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "Write tests");
  await page.locator("#new-card").fill("Ship it");
  await page.locator("#new-card").press("Enter");
  const todo = await texts(page, "todo");
  check("Add and Enter both add to the end of To do", same(todo, ["Write tests", "Ship it"]), JSON.stringify(todo));
  check("To do count is 2", (await count(page, "todo")) === "2");
  check("the text box is cleared", (await page.locator("#new-card").inputValue()) === "");
});
