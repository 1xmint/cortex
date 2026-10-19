import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  await page.click("#plus");
  const text = (await page.textContent("#count"))?.trim();
  check("one click shows 1", text === "1", `count was ${text}`);
}, { timeoutMs: 15000 });
