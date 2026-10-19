import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, rows, row, text } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "Cinema", amount: "9", date: "2026-03-06", category: "fun" });
  await submit(page, { description: "Rent", amount: "500", date: "2026-03-01", category: "home" });

  await row(page, "Cinema").locator(".edit").click({ timeout: 2000 });
  check("edit fills the description", (await page.inputValue("#description")) === "Cinema");
  check("edit fills the amount", Number(await page.inputValue("#amount")) === 9);
  check("edit fills the date", (await page.inputValue("#date")) === "2026-03-06");
  check("edit fills the category", (await page.inputValue("#category")) === "fun");

  await submit(page, { description: "Cinema and popcorn", amount: "14" });
  const got = await rows(page);
  check("edit changes, not adds", got.length === 2, JSON.stringify(got));
  const changed = got.find((r) => r.description === "Cinema and popcorn");
  check("the change is shown", changed?.amount === "14.00" && changed?.category === "fun", JSON.stringify(got));
  check("the total follows the edit", (await text(page, "#total")) === "514.00", await text(page, "#total"));

  await row(page, "Rent").locator(".delete").click({ timeout: 2000 });
  check("delete removes it", (await rows(page)).map((r) => r.description).join() === "Cinema and popcorn");
  check("the total follows the delete", (await text(page, "#total")) === "14.00", await text(page, "#total"));

  await submit(page, { description: "Snack", amount: "1", date: "2026-03-07" });
  check("the form adds again after an edit", (await rows(page)).length === 2);
});
