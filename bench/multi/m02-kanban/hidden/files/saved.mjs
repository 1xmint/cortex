import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, press, edit, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  for (const t of ["Alpha", "Beta", "Gamma"]) await add(page, t);
  await press(page, "Beta", "right");
  await edit(page, "Gamma", "Delta", "Enter");
  await open(page, ctx);
  const todo = await texts(page, "todo");
  check("To do survives a reload in order", same(todo, ["Alpha", "Delta"]), JSON.stringify(todo));
  check("Doing survives a reload", same(await texts(page, "doing"), ["Beta"]));
  await press(page, "Alpha", "delete");
  await open(page, ctx);
  check("a delete survives a reload", same(await texts(page, "todo"), ["Delta"]));
});
