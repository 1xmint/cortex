import { withPage, check } from "./lib/cortex-check.mjs";
import { open, press, still, slides } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await page.uncheck("#animations");
  await open(page, ctx);
  check("off survives a reload", !(await page.isChecked("#animations")));
  await press(page, "ArrowLeft");
  await press(page, "ArrowRight");
  check("and tiles stay still after it", await still(page));

  await page.check("#animations");
  await open(page, ctx);
  check("on survives a reload", await page.isChecked("#animations"));
  check("and tiles slide after it", await slides(page));
});
