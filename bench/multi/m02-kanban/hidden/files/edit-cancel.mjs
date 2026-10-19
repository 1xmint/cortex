import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, edit, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "Original");
  await edit(page, "Original", "Changed", "Escape");
  let todo = await texts(page, "todo");
  check("Escape keeps the old text", same(todo, ["Original"]), JSON.stringify(todo));
  check("Escape closes the edit box", (await page.locator(".edit-input").count()) === 0);
  await edit(page, "Original", "   ", "Enter");
  todo = await texts(page, "todo");
  check("saving empty text keeps the old text", same(todo, ["Original"]), JSON.stringify(todo));
  await edit(page, "Original", "  Trimmed  ", "Enter");
  todo = await texts(page, "todo");
  check("edited text is trimmed", same(todo, ["Trimmed"]), JSON.stringify(todo));
});
