import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, click, pixel, near, blank } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await stroke(page, [[50, 50], [150, 50]]);
  await stroke(page, [[50, 150], [150, 150]]);

  await click(page, "#undo");
  check("undo takes back the last stroke", blank(await pixel(page, 100, 150)), String(await pixel(page, 100, 150)));
  check("and only that one", near(await pixel(page, 100, 50), [0, 0, 0]));

  await click(page, "#redo");
  check("redo puts it back", near(await pixel(page, 100, 150), [0, 0, 0]), String(await pixel(page, 100, 150)));
  check("and keeps the first", near(await pixel(page, 100, 50), [0, 0, 0]));
});
