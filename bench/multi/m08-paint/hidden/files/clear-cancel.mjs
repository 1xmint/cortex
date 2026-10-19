import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, clear, click, allBlank } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await stroke(page, [[50, 50], [350, 250]]);
  check("clear asks first", await clear(page, false));
  check("saying no keeps the drawing", !(await allBlank(page)));
  check("clear asks again", await clear(page, true));
  check("saying yes empties it", await allBlank(page));
  await click(page, "#undo");
  check("a clear can be undone", !(await allBlank(page)));
});
