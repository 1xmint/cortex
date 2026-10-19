import { withPage, check } from "./lib/cortex-check.mjs";
import { open, stroke, click, size, pixel, near, active } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, 20);
  await stroke(page, [[50, 100], [350, 100]]);

  await click(page, "#eraser");
  check("the eraser shows as chosen", (await active(page, "#eraser")) && !(await active(page, "#pen")));
  await stroke(page, [[150, 100], [250, 100]]);
  check("the eraser paints white", near(await pixel(page, 200, 100), [255, 255, 255]), String(await pixel(page, 200, 100)));
  check("only where it went", near(await pixel(page, 100, 100), [0, 0, 0]), String(await pixel(page, 100, 100)));

  await click(page, "#pen");
  await stroke(page, [[200, 50], [200, 150]]);
  check("the pen draws again", near(await pixel(page, 200, 100), [0, 0, 0]), String(await pixel(page, 200, 100)));
});
