import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, rows, row, text, setMonth, chartTotals } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "Gym", amount: "35", date: "2026-03-01", category: "fun" });
  await submit(page, { description: "Bills", amount: "80.40", date: "2026-03-11", category: "home" });
  await submit(page, { description: "Gone", amount: "1", date: "2026-03-12" });
  await row(page, "Gone").locator(".delete").click({ timeout: 2000 });
  const before = await rows(page);

  await open(page, ctx);
  const after = await rows(page);
  check("the list survives a reload", JSON.stringify(after) === JSON.stringify(before), JSON.stringify(after));
  check("so does the total", (await text(page, "#total")) === "115.40", await text(page, "#total"));
  await setMonth(page, "2026-03");
  check("so do the month total", (await text(page, "#month-total")) === "115.40", await text(page, "#month-total"));
  const totals = await chartTotals(page);
  check("and the category totals", totals.fun === "35.00" && totals.home === "80.40", JSON.stringify(totals));

  await submit(page, { description: "After", amount: "2", date: "2026-03-13" });
  check("adding after a reload keeps the old ones", (await rows(page)).length === 3);
  await open(page, ctx);
  check("and is saved too", (await rows(page)).length === 3);
});
