import { withPage, check } from "./lib/cortex-check.mjs";
import { open, flip, count, size, visible, win } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const n = await count(page);
  for (let i = 0; i < n - 2; i += 2) {
    await flip(page, i);
    await flip(page, i + 1);
  }
  check("not won with a pair left", !(await visible(page, "#won")));
  await flip(page, n - 2);
  await flip(page, n - 1);
  check("medium can be won", await visible(page, "#won"));

  await size(page, "hard", 36);
  check("a new size hides the win message", !(await visible(page, "#won")));
  await win(page);
  check("hard can be won", await visible(page, "#won"));
});
