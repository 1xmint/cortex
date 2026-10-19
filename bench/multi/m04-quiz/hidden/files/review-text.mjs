import { withPage, check } from "./lib/cortex-check.mjs";
import { open, served, finish } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const qs = await served(page);
  await page.locator("#start").click({ timeout: 2000 });
  // Answer everything wrong, so the review must show answers not given.
  const asked = await finish(page, qs, (q) => (q.answer + 1) % 4);

  const lines = (await page.locator("#review li").allTextContents()).map((t) => t.trim());
  check("one line per question", lines.length === asked.length, lines);
  asked.forEach((shown, i) => {
    const q = qs.find((q) => q.question === shown);
    const line = lines[i] ?? "";
    check(`line ${i + 1} names its question`, line.includes(q.question), line);
    check(`line ${i + 1} gives the correct answer`, line.includes(q.choices[q.answer]), line);
  });
});
