import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit, rows, row } from "./common.mjs";

const order = async (page) => (await rows(page)).map((r) => r.description).join(",");

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "B", amount: "1", date: "2026-03-05" });
  await submit(page, { description: "A", amount: "1", date: "2026-03-09" });
  await submit(page, { description: "C", amount: "1", date: "2026-03-05" });
  await submit(page, { description: "D", amount: "1", date: "2026-02-20" });
  check("newest date first, same date in the order added", (await order(page)) === "A,B,C,D", await order(page));

  await row(page, "D").locator(".edit").click({ timeout: 2000 });
  await submit(page, { date: "2026-03-30" });
  check("an edited date moves the expense", (await order(page)) === "D,A,B,C", await order(page));

  const today = await page.evaluate(() => {
    const d = new Date();
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  });
  await submit(page, { description: "Now", amount: "2" });
  const now = (await rows(page)).find((r) => r.description === "Now");
  check("an empty date still means today", now?.date === today, `${now?.date} vs ${today}`);
});
