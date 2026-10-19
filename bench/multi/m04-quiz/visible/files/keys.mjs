import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);

  await page.keyboard.press("Enter");
  check("Enter starts the quiz", (await text(page, "#question")) === qs[0].question);

  await page.keyboard.press(String(qs[0].answer + 1));
  check("number key answers", (await text(page, "#question")) === qs[1].question);
  check("number key picks that choice", (await text(page, "#score")) === "1");

  await page.keyboard.press(String(((qs[1].answer + 1) % 4) + 1));
  check("a wrong number key does not score", (await text(page, "#score")) === "1");
  check("and moves on", (await text(page, "#question")) === qs[2].question);
});
