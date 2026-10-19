import { withPage, check } from "./lib/cortex-check.mjs";
await withPage(async (page) => {
  const text = (await page.textContent("#count"))?.trim();
  check("count starts at 0", text === "0", `count was ${text}`);
}, { timeoutMs: 15000 });
