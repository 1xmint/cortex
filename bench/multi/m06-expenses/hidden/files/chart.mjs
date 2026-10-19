import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, row, setMonth, chartTotals } from "./common.mjs";

const drawing = (page) => page.locator("#chart").evaluate((c) => c.toDataURL());
const same = (got, want) => JSON.stringify(got) === JSON.stringify(want);

await withPage(async (page, ctx) => {
  await open(page, ctx);
  check("the chart is a canvas", (await page.locator("canvas#chart").count()) === 1);
  await setMonth(page, "2026-03");
  const empty = await drawing(page);
  const zero = { food: "0.00", transport: "0.00", home: "0.00", fun: "0.00", other: "0.00" };
  check("an empty month has zero totals", same(await chartTotals(page), zero), JSON.stringify(await chartTotals(page)));

  await submit(page, { description: "Groceries", amount: "30.25", date: "2026-03-02", category: "food" });
  await submit(page, { description: "Pizza", amount: "12", date: "2026-03-15", category: "food" });
  await submit(page, { description: "Fuel", amount: "40", date: "2026-03-20", category: "transport" });
  await submit(page, { description: "Lamp", amount: "25", date: "2026-04-01", category: "home" });
  const march = { ...zero, food: "42.25", transport: "40.00" };
  check("March totals per category", same(await chartTotals(page), march), JSON.stringify(await chartTotals(page)));
  const marchDrawing = await drawing(page);
  check("the chart draws the totals", marchDrawing !== empty);

  await setMonth(page, "2026-04");
  check("the month change updates the totals", same(await chartTotals(page), { ...zero, home: "25.00" }), JSON.stringify(await chartTotals(page)));
  check("and the drawing", (await drawing(page)) !== marchDrawing);

  await setMonth(page, "2026-03");
  await row(page, "Fuel").locator(".delete").click({ timeout: 2000 });
  check("a delete updates the totals", same(await chartTotals(page), { ...zero, food: "42.25" }), JSON.stringify(await chartTotals(page)));
  await row(page, "Pizza").locator(".edit").click({ timeout: 2000 });
  await submit(page, { category: "fun" });
  check("an edit updates the totals", same(await chartTotals(page), { ...zero, food: "30.25", fun: "12.00" }), JSON.stringify(await chartTotals(page)));
});
