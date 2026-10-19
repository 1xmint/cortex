import { withPage, check } from "./lib/cortex-check.mjs";
import { load, press, still, slides } from "./common.mjs";

const rows = [
  [2, 0, 0, 0],
  [0, 0, 0, 0],
  [0, 0, 4, 0],
  [0, 0, 0, 0],
];

await withPage(async (page, ctx) => {
  await load(page, ctx, rows);
  check("the switch starts on", await page.isChecked("#animations"));
  check("tiles slide while it is on", await slides(page));

  await page.uncheck("#animations");
  await press(page, "ArrowRight");
  check("switched off, tiles have no transition or animation", await still(page));

  await page.check("#animations");
  await press(page, "ArrowLeft");
  check("switched back on, tiles slide again", await slides(page));
});
