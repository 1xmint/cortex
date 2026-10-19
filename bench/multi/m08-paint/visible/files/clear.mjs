import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, clear, allBlank } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await stroke(page, [[50, 50], [350, 250]]);
  check("something is drawn", !(await allBlank(page)));
  check("clear asks first", await clear(page, true));
  check("agreeing to clear empties the canvas", await allBlank(page));
});
