import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, rows, text, visible } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "Lunch", amount: "12.5", date: "2026-03-04" });
  const got = await rows(page);
  check("an expense is added", got.length === 1 && got[0].description === "Lunch", JSON.stringify(got));
  check("it shows its date and amount", got[0]?.date === "2026-03-04" && got[0]?.amount === "12.50", JSON.stringify(got));
  check("it is filed under the default category", got[0]?.category === "food", JSON.stringify(got));
  check("the form is cleared", (await page.inputValue("#description")) === "" && (await page.inputValue("#amount")) === "");

  await submit(page, { description: "   ", amount: "3" });
  check("an empty description is refused", (await rows(page)).length === 1);
  check("and says why", await visible(page, "#error"));
  await submit(page, { description: "Bus", amount: "0" });
  check("a zero amount is refused", (await rows(page)).length === 1);

  await submit(page, { description: "Bus", amount: "2.75", date: "2026-03-05" });
  check("a good submit adds", (await rows(page)).length === 2);
  check("and hides the error", !(await visible(page, "#error")));
  check("the total adds up", (await text(page, "#total")) === "15.25", await text(page, "#total"));
});
