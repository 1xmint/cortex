import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, row } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  const values = await page.locator("#category option").evaluateAll((os) => os.map((o) => o.value));
  check("the five categories, food first", values.join(",") === "food,transport,home,fun,other", values.join(","));
  check("food is chosen by default", (await page.inputValue("#category")) === "food");

  await submit(page, { description: "Train", amount: "4", date: "2026-03-02", category: "transport" });
  await submit(page, { description: "Bread", amount: "2", date: "2026-03-03" });
  check("an expense shows its category", (await row(page, "Train").locator(".category").textContent())?.trim() === "transport");
  check("the default is food", (await row(page, "Bread").locator(".category").textContent())?.trim() === "food");
});
