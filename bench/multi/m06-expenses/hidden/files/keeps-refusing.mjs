import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, rows, row, text, visible } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "Taxi", amount: "20", date: "2026-03-03", category: "transport" });

  await row(page, "Taxi").locator(".edit").click({ timeout: 2000 });
  await submit(page, { description: "  " });
  check("an edit to an empty description is refused", await visible(page, "#error"));
  let got = await rows(page);
  check("and the expense is unchanged", got.length === 1 && got[0].description === "Taxi", JSON.stringify(got));

  await submit(page, { description: "Taxi home", amount: "-5" });
  check("an edit to a negative amount is refused", (await rows(page))[0]?.amount === "20.00");

  await submit(page, { amount: "22" });
  got = await rows(page);
  check("the edit then saves", got.length === 1 && got[0].description === "Taxi home" && got[0].amount === "22.00", JSON.stringify(got));
  check("and hides the error", !(await visible(page, "#error")));
  check("the total follows", (await text(page, "#total")) === "22.00", await text(page, "#total"));
});
