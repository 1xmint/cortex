import { withPage, check } from "./lib/cortex-check.mjs";
import { open, size, pixel, near, blank } from "./common.mjs";

await withPage(async (page, ctx) => {
  await open(page, ctx);
  await size(page, 20);
  const box = await page.locator("#canvas").boundingBox();
  // Start inside, leave the canvas, let go outside, then come back in without pressing.
  await page.mouse.move(box.x + 100, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x + 100, box.y + box.height + 40, { steps: 8 });
  await page.mouse.up();
  await page.mouse.move(box.x + 300, box.y + 200, { steps: 8 });
  check("the stroke drew while inside", near(await pixel(page, 100, 200), [0, 0, 0]), String(await pixel(page, 100, 200)));
  check("letting go outside ends it", blank(await pixel(page, 300, 200)), String(await pixel(page, 300, 200)));

  // A big dot is round and about 20 wide.
  await page.mouse.move(box.x + 300, box.y + 80);
  await page.mouse.down();
  await page.mouse.up();
  check("a size 20 dot reaches 7 pixels out", near(await pixel(page, 307, 80), [0, 0, 0]) && near(await pixel(page, 300, 73), [0, 0, 0]));
  check("but not 13", blank(await pixel(page, 313, 80)) && blank(await pixel(page, 300, 67)));
  check("and it is round", blank(await pixel(page, 308, 72)));
});
