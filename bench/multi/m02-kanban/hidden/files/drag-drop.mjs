import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, count, card, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "First");
  await add(page, "Second");
  await card(page, "First").dragTo(page.locator("#col-done"), { timeout: 2000 });
  check("dragging moves a card to Done", same(await texts(page, "done"), ["First"]));
  check("it leaves To do", same(await texts(page, "todo"), ["Second"]));
  await card(page, "Second").dragTo(page.locator("#col-done"), { timeout: 2000 });
  const done = await texts(page, "done");
  check("a dropped card goes to the end", same(done, ["First", "Second"]), JSON.stringify(done));
  check("counts follow", (await count(page, "todo")) === "0" && (await count(page, "done")) === "2");
  await card(page, "First").dragTo(page.locator("#col-doing"), { timeout: 2000 });
  check("drag works into Doing", same(await texts(page, "doing"), ["First"]));
});
