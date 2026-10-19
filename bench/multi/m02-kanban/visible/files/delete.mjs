import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, press, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "Keep");
  await add(page, "Remove");
  await press(page, "Remove", "delete");
  const todo = await texts(page, "todo");
  check("delete removes only that card", same(todo, ["Keep"]), JSON.stringify(todo));
  check("To do count is 1", (await count(page, "todo")) === "1");
});
