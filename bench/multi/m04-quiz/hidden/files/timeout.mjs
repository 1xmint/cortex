import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text, visible, finish } from "./common.mjs";

await withPage(async (page, ctx) => {
  await page.clock.install();
  await open(page, ctx);
  const qs = await served(page);
  await page.locator("#start").click({ timeout: 2000 });

  await page.clock.runFor(3000);
  check("timer counts down", (await text(page, "#timer")) === "7");
  check("still on the first question", (await text(page, "#question")) === qs[0].question);

  await page.clock.runFor(7500);
  check("time running out moves on", (await text(page, "#question")) === qs[1].question);
  check("a timed-out question does not score", (await text(page, "#score")) === "0");
  check("the timer starts again", (await text(page, "#timer")) === "10");

  await finish(page, qs, (q) => q.answer);
  check("results show", await visible(page, "#results"));
  check("the timeout counts as wrong", (await text(page, "#final-score")) === `${qs.length - 1} / ${qs.length}`);
  const first = await page.locator("#review li").first().getAttribute("class");
  check("review marks it wrong", /\bwrong\b/.test(first ?? ""), first);
});
