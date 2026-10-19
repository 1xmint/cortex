import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, edit, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "Old name");
  await edit(page, "Old name", "New name", "Enter");
  const todo = await texts(page, "todo");
  check("Enter saves the edited text", same(todo, ["New name"]), JSON.stringify(todo));
  check("the edit box goes away", (await page.locator(".edit-input").count()) === 0);
});
