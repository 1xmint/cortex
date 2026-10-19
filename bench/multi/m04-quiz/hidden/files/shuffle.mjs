import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, finish } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  const fileOrder = JSON.stringify(qs.map((q) => q.question));
  const sorted = JSON.stringify(qs.map((q) => q.question).sort());

  await page.locator("#start").click({ timeout: 2000 });
  const first = await finish(page, qs, (q) => q.answer);
  check("the first quiz is in the file's order", JSON.stringify(first) === fileOrder, first);

  // Ten restarts: each asks every question once, and the order changes.
  let differs = 0;
  for (let round = 0; round < 10; round++) {
    await page.locator("#restart").click({ timeout: 2000 });
    const asked = await finish(page, qs, (q) => q.answer);
    check(`restart ${round + 1} asks every question once`, JSON.stringify([...asked].sort()) === sorted, asked);
    if (JSON.stringify(asked) !== fileOrder) differs++;
  }
  check("restarts shuffle the order", differs > 0, `${differs} of 10 differ from the file`);
});
