import { withPage, check } from "./lib/cortex-check.mjs";
import { open, add, texts, press, same } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await add(page, "A");
  await add(page, "B");
  await press(page, "A", "left");
  check("left does nothing in To do", same(await texts(page, "todo"), ["A", "B"]));
  await press(page, "A", "right");
  await press(page, "B", "right");
  check("moved cards go to the end", same(await texts(page, "doing"), ["A", "B"]));
  await press(page, "A", "left");
  await press(page, "A", "right");
  const doing = await texts(page, "doing");
  check("moving back and forth puts it last", same(doing, ["B", "A"]), JSON.stringify(doing));
});
