import fs from "node:fs";
import { withPage, check } from "./lib/cortex-check.mjs";
import { open, submit } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await submit(page, { description: "Coffee", amount: "3.5", date: "2026-03-02", category: "food" });
  await submit(page, { description: "Paint, brushes", amount: "18", date: "2026-03-09", category: "home" });
  await submit(page, { description: 'The "big" show', amount: "45.99", date: "2026-03-05", category: "fun" });

  const [download] = await Promise.all([
    page.waitForEvent("download", { timeout: 5000 }),
    page.locator("#export").click({ timeout: 2000 }),
  ]);
  check("the file is expenses.csv", download.suggestedFilename() === "expenses.csv", download.suggestedFilename());
  const lines = fs.readFileSync(await download.path(), "utf8").replace(/\r\n/g, "\n").trimEnd().split("\n");
  const want = [
    "date,description,category,amount",
    '2026-03-09,"Paint, brushes",home,18.00',
    '2026-03-05,"The ""big"" show",fun,45.99',
    "2026-03-02,Coffee,food,3.50",
  ];
  check("the header comes first", lines[0] === want[0], lines[0]);
  check("one line per expense", lines.length === want.length, String(lines.length));
  check("rows in list order, amounts in two decimals", lines[3] === want[3] && lines[1]?.startsWith("2026-03-09"), lines.join(" | "));
  check("a comma is quoted", lines[1] === want[1], lines[1]);
  check("a double quote is doubled", lines[2] === want[2], lines[2]);
});
