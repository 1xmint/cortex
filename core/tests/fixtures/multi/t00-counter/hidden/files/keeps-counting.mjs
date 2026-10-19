import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  for (let i = 0; i < 5; i++) await page.click("#plus");
  const text = (await page.textContent("#count"))?.trim();
  check("five clicks show 5", text === "5", `count was ${text}`);
}, { timeoutMs: 15000 });
