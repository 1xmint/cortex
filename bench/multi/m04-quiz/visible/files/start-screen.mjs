import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text, choices, visible } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  check("no question before the start", !(await visible(page, "#question")));

  await page.locator("#start").click({ timeout: 2000 });
  check("start button hides", !(await visible(page, "#start")));
  check("first question shows", (await text(page, "#question")) === qs[0].question);
  const shown = await choices(page);
  check("its four choices show in order", JSON.stringify(shown) === JSON.stringify(qs[0].choices), shown);
  check("score starts at 0", (await text(page, "#score")) === "0");
  check("timer starts at 10", (await text(page, "#timer")) === "10");
  check("results stay hidden", !(await visible(page, "#results")));
});
