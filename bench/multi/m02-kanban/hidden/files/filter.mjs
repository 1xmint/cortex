import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, press, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  for (const t of ["Buy milk", "Write code", "MILK run"]) await add(page, t);
  await press(page, "Write code", "right");
  await page.locator("#filter").fill("milk");
  const todo = await texts(page, "todo");
  check("filter shows matches, ignoring case", same(todo, ["Buy milk", "MILK run"]), JSON.stringify(todo));
  check("filter hides cards that do not match", same(await texts(page, "doing"), []));
  check("counts still count every card", (await count(page, "todo")) === "2" && (await count(page, "doing")) === "1");
  await page.locator("#filter").fill("");
  check("clearing the filter shows every card", same(await texts(page, "doing"), ["Write code"]));
});
