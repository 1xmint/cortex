import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text, pick } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  await page.locator("#start").click({ timeout: 2000 });

  await pick(page, qs[0].answer);
  check("a right answer moves on", (await text(page, "#question")) === qs[1].question);
  check("a right answer scores", (await text(page, "#score")) === "1");

  await pick(page, (qs[1].answer + 1) % 4);
  check("a wrong answer moves on", (await text(page, "#question")) === qs[2].question);
  check("a wrong answer does not score", (await text(page, "#score")) === "1");
});
