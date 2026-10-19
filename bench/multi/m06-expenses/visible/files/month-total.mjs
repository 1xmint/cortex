import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, text, setMonth } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const current = await page.evaluate(() => {
    const d = new Date();
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
  });
  check("the month starts on this month", (await page.inputValue("#month")) === current, await page.inputValue("#month"));

  await submit(page, { description: "Tea", amount: "3.10", date: "2026-03-10" });
  await submit(page, { description: "Cake", amount: "4.20", date: "2026-03-28" });
  await submit(page, { description: "Soup", amount: "5", date: "2026-04-01" });
  await setMonth(page, "2026-03");
  check("March adds up", (await text(page, "#month-total")) === "7.30", await text(page, "#month-total"));
  await setMonth(page, "2026-04");
  check("April adds up", (await text(page, "#month-total")) === "5.00", await text(page, "#month-total"));
  await setMonth(page, "2026-05");
  check("an empty month is 0.00", (await text(page, "#month-total")) === "0.00", await text(page, "#month-total"));
});
