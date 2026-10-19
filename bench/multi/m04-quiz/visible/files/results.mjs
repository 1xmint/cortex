import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, text, visible, finish } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  await page.locator("#start").click({ timeout: 2000 });

  // Right on every question but the third and the last.
  const wrong = new Set([qs[2].question, qs[qs.length - 1].question]);
  await finish(page, qs, (q) => (wrong.has(q.question) ? (q.answer + 1) % 4 : q.answer));

  check("results show", await visible(page, "#results"));
  check("questions are gone", !(await visible(page, "#question")));
  const right = qs.length - 2;
  check("final score is right / total", (await text(page, "#final-score")) === `${right} / ${qs.length}`);
  const marks = await page.locator("#review li").evaluateAll((lis) =>
    lis.map((li) => (li.classList.contains("right") ? "right" : li.classList.contains("wrong") ? "wrong" : "?")),
  );
  const want = qs.map((q) => (wrong.has(q.question) ? "wrong" : "right"));
  check("review marks each answer", JSON.stringify(marks) === JSON.stringify(want), marks);
});
